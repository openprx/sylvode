//! Independent block-share service contracts. Not registered as HTTP routes yet.
use crate::error::ApiError;
use base64::Engine as _;
use collab_core::block_projection::SharedProjection;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharePermission {
    View,
    Edit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudienceKind {
    Workspace,
    Users,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudienceInput {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub audience: AudienceKind,
    #[serde(default)]
    pub recipient_user_ids: Vec<Uuid>,
    pub permission: SharePermission,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Block,
    CurrentSubtree,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub expected_frontier: String,
    pub root_block_cid: String,
    pub selection: Selection,
    pub audiences: Vec<AudienceInput>,
}
#[derive(Clone, Debug, Serialize)]
pub struct RecipientView {
    pub display_context: DisplayContext,
    pub projection: SharedProjection,
    pub effective_permission: SharePermission,
}

pub fn validate_audience_shapes(audiences: &[AudienceInput]) -> Result<(), ApiError> {
    if audiences.len() != 1 {
        return Err(ApiError::invalid_update("share audience limit"));
    }
    for audience in audiences {
        let unique = audience.recipient_user_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != audience.recipient_user_ids.len() || unique.len() > 128 || unique.contains(&Uuid::nil()) {
            return Err(ApiError::invalid_update("invalid explicit share recipients"));
        }
        match audience.audience {
            AudienceKind::Workspace if audience.project_id.is_some() || !unique.is_empty() => {
                return Err(ApiError::invalid_update(
                    "project placement requires explicit recipients",
                ));
            }
            AudienceKind::Users if unique.is_empty() => {
                return Err(ApiError::invalid_update("explicit share recipients required"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Mirrors actual `get_project`'s project→workspace binding and `UserSession` membership policy.
/// Call again inside the publishing transaction after destination workspace/epoch fences.
pub async fn validate_destination_policy<C: ConnectionTrait>(
    db: &C,
    publisher: Uuid,
    audiences: &[AudienceInput],
) -> Result<(), ApiError> {
    validate_audience_shapes(audiences)?;
    for audience in audiences {
        let manager=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT user_id FROM workspace_members WHERE workspace_id=$1 AND user_id=$2 AND role IN ('owner','admin')",
            vec![audience.workspace_id.into(),publisher.into()])).await?;
        if manager.is_none() {
            return Err(ApiError::forbidden("destination publishing authority required"));
        }
        if let Some(project) = audience.project_id
            && db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM projects WHERE id=$1 AND workspace_id=$2",
                    vec![project.into(), audience.workspace_id.into()],
                ))
                .await?
                .is_none()
        {
            return Err(ApiError::not_found("destination project unavailable"));
        }
        for recipient in &audience.recipient_user_ids {
            if db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT m.user_id FROM workspace_members m JOIN users u ON u.id=m.user_id AND u.entity_type='human' AND u.is_active WHERE m.workspace_id=$1 AND m.user_id=$2",
                    vec![audience.workspace_id.into(), (*recipient).into()],
                ))
                .await?
                .is_none()
            {
                return Err(ApiError::forbidden("recipient lacks destination access"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_placement_is_never_an_implicit_workspace_audience() {
        let mut audience = AudienceInput {
            workspace_id: Uuid::from_u128(1),
            project_id: Some(Uuid::from_u128(2)),
            audience: AudienceKind::Workspace,
            recipient_user_ids: vec![],
            permission: SharePermission::View,
        };
        assert!(validate_audience_shapes(&[audience.clone()]).is_err());
        audience.audience = AudienceKind::Users;
        assert!(validate_audience_shapes(&[audience.clone()]).is_err());
        audience.recipient_user_ids = vec![Uuid::from_u128(3)];
        assert!(validate_audience_shapes(&[audience]).is_ok());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceView {
    pub root_block_cid: String,
    pub selection: Selection,
    pub share_id: Uuid,
    pub state: String,
    pub revision: i64,
    pub member_count: usize,
    pub audiences: Vec<AudienceInput>,
}

/// Source-only publication. No source snapshot is ever persisted in the share or receipt.
async fn confirm_completed(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    proof: &super::scope::AuthorizationProof,
    identity: &super::scope::ObjectIdentity,
    share: Uuid,
    audiences: &[AudienceInput],
) -> Result<SourceView, ApiError> {
    let tx = db.begin().await?;
    let workspaces = audiences
        .iter()
        .map(|audience| audience.workspace_id)
        .chain(proof.audience_revisions.iter().map(|(workspace, _)| *workspace))
        .chain(identity.workspace_id)
        .collect::<BTreeSet<_>>();
    for workspace in workspaces {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
            vec![workspace.into()],
        ))
        .await?;
        super::collab::authz::lock_epoch_for_share(&tx, workspace).await?;
    }
    super::scope::fence(&tx, proof).await?;
    validate_destination_policy(&tx, actor, audiences).await?;
    if tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_block_shares WHERE id=$1 AND source_page_id=$2 FOR SHARE",
            vec![share.into(), identity.id.into()],
        ))
        .await?
        .is_none()
    {
        return Err(unavailable());
    }
    let current = source_view(&tx, share).await?;
    tx.commit().await?;
    Ok(current)
}

async fn publish(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    key: &str,
    input: CreateInput,
    existing: Option<Uuid>,
) -> Result<SourceView, ApiError> {
    use super::{
        collab::authz::{self, PermissionLevel},
        scope,
    };
    let proof = scope::authorize(db, source, actor, PermissionLevel::FullAccess).await?;
    let identity = scope::identity(db, source).await?;
    if identity.object_type != "page" || !proof.resource_active {
        return Err(ApiError::not_found("source Page unavailable"));
    }
    let body = serde_json::json!({"operation":if existing.is_some(){"block_share_republish"}else{"block_share_create"},"share_id":existing,"expected_frontier":input.expected_frontier,"source":source,"root_block_cid":input.root_block_cid,"selection":input.selection,"audiences":input.audiences});
    if let Some(result) = super::scope_commands::replay(db, identity.scope_id, actor, key, &body).await? {
        let completed: SourceView = serde_json::from_value(result).map_err(|_| ApiError::internal())?;
        return confirm_completed(db, actor, &proof, &identity, completed.share_id, &input.audiences).await;
    }
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("invalid shared command identity"));
    }
    validate_destination_policy(db, actor, &input.audiences).await?;
    super::block_anchors::complete_registration(db, source).await?;
    let document = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id=$1",
            vec![source.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("source Page unavailable"))?
        .try_get::<Uuid>("", "id")?;
    let boot = super::collab::bootstrap::load(db, document).await?;
    let engine = super::document::engine_from_boot(&boot)?;
    if !engine
        .has_editor_doc()
        .map_err(|error| super::command::map_collab_error(&error))?
    {
        return Err(ApiError::policy_rejected_with_details(
            "enable Page editor before sharing body blocks",
            serde_json::json!({"reason":"explicit_editor_initialization_required"}),
        ));
    }
    let semantic = engine
        .semantic_snapshot()
        .map_err(|error| super::command::map_collab_error(&error))?;
    let prepared = super::block_registry::prepare(&semantic)?;
    let reviewed = base64::engine::general_purpose::STANDARD
        .decode(&input.expected_frontier)
        .map_err(|_| ApiError::invalid_update("invalid reviewed Page frontier"))?;
    if !super::frontier::client_matches(&reviewed, &boot.head_frontier)? || !engine.matches_frontier(&reviewed) {
        return Err(ApiError::policy_rejected("Page changed after selection review"));
    }
    let root = prepared
        .iter()
        .find(|block| block.physical_cid == input.root_block_cid)
        .ok_or_else(|| ApiError::invalid_update("selected block unavailable"))?;
    let mut selected = Vec::new();
    for block in &prepared {
        let mut include = block.physical_cid == root.physical_cid;
        if !include && matches!(input.selection, Selection::CurrentSubtree) {
            let mut parent = block.parent_physical_cid.as_deref();
            let mut seen = BTreeSet::new();
            while let Some(cid) = parent {
                if !seen.insert(cid) {
                    return Err(ApiError::invalid_update("cyclic Page structure"));
                }
                if cid == root.physical_cid {
                    include = true;
                    break;
                }
                parent = prepared
                    .iter()
                    .find(|candidate| candidate.physical_cid == cid)
                    .and_then(|candidate| candidate.parent_physical_cid.as_deref());
            }
        }
        if include {
            selected.push(block);
        }
    }
    for block in &selected {
        let node = semantic
            .nodes
            .get(block.physical_cid.as_str())
            .ok_or_else(unavailable)?;
        if !matches!(
            node.properties.get("block_type").map_or("paragraph", String::as_str),
            "paragraph"
                | "heading"
                | "bulletItem"
                | "orderedItem"
                | "todoItem"
                | "blockquote"
                | "toggle"
                | "codeBlock"
                | "divider"
                | "image"
                | "attachment"
                | "childPage"
        ) {
            return Err(ApiError::invalid_update(
                "selected legacy block has no typed share projection",
            ));
        }
    }
    if selected.len() > collab_core::block_projection::SHARE_MEMBER_MAX {
        return Err(ApiError::invalid_update("selected subtree exceeds share member limit"));
    }
    selected.sort_by(|left, right| {
        left.order_key
            .cmp(&right.order_key)
            .then(left.physical_cid.cmp(&right.physical_cid))
    });
    let tx = db.begin().await?;
    let workspaces = input
        .audiences
        .iter()
        .map(|audience| audience.workspace_id)
        .chain(proof.audience_revisions.iter().map(|(workspace, _)| *workspace))
        .chain(identity.workspace_id)
        .collect::<BTreeSet<_>>();
    for workspace in &workspaces {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
            vec![(*workspace).into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_workspace_settings(workspace_id) VALUES($1) ON CONFLICT DO NOTHING",
            vec![(*workspace).into()],
        ))
        .await?;
        authz::lock_epoch_for_share(&tx, *workspace).await?;
    }
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    validate_destination_policy(&tx, actor, &input.audiences).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, actor, key, &body).await? {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    let head = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq,head_frontier FROM collab_documents WHERE id=$1 FOR UPDATE",
            vec![document.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if !super::frontier::same(&reviewed, &head.try_get::<Vec<u8>>("", "head_frontier")?)? {
        return Err(ApiError::policy_rejected("source Page changed; select blocks again"));
    }
    let seq = head.try_get::<i64>("", "head_seq")?;
    // The completion loop registered the Page in capped steps before this transaction, so only
    // changes made since then are registered here, completely: the share needs every identity.
    super::block_registry::reconcile(
        &tx,
        identity.scope_id,
        source,
        seq,
        &prepared,
        super::block_registry::RegistrationAllowance::Complete,
    )
    .await?;
    let root_logical=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT logical_id FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 AND state='active'",vec![identity.scope_id.into(),source.into(),input.root_block_cid.clone().into()])).await?.ok_or_else(unavailable)?.try_get::<Uuid>("","logical_id")?;
    let share_id = existing.unwrap_or_else(Uuid::new_v4);
    let mut old_members = std::collections::BTreeMap::new();
    let mut old_aliases = std::collections::BTreeMap::new();
    let mut placements = std::collections::BTreeMap::new();
    if existing.is_some() {
        let previous = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT source_page_id,state,root_logical_id FROM flow_block_shares WHERE id=$1 FOR UPDATE",
                vec![share_id.into()],
            ))
            .await?
            .ok_or_else(unavailable)?;
        let previous_state = previous.try_get::<String>("", "state")?;
        if previous.try_get::<Uuid>("", "root_logical_id")? != root_logical
            || previous_state == "revoked"
            || (previous.try_get::<Uuid>("", "source_page_id")? != source && previous_state != "suspended")
        {
            return Err(unavailable());
        }
        let target = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id,workspace_id,project_id FROM flow_block_share_placements WHERE share_id=$1",
                vec![share_id.into()],
            ))
            .await?
            .ok_or_else(unavailable)?;
        let audience = input.audiences.first().ok_or_else(unavailable)?;
        if target.try_get::<Uuid>("", "workspace_id")? != audience.workspace_id
            || target.try_get::<Option<Uuid>>("", "project_id")? != audience.project_id
        {
            return Err(ApiError::invalid_update("a different target requires a new share"));
        }
        placements.insert(
            (audience.workspace_id, audience.project_id),
            target.try_get::<Uuid>("", "id")?,
        );
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT member_id,logical_id FROM flow_block_share_members WHERE share_id=$1",
                vec![share_id.into()],
            ))
            .await?
        {
            old_members.insert(
                row.try_get::<Uuid>("", "logical_id")?,
                row.try_get::<Uuid>("", "member_id")?,
            );
        }
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT member_id,asset_id,alias_id FROM flow_block_share_asset_aliases WHERE share_id=$1",
                vec![share_id.into()],
            ))
            .await?
        {
            old_aliases.insert(
                (
                    row.try_get::<Uuid>("", "member_id")?,
                    row.try_get::<Uuid>("", "asset_id")?,
                ),
                row.try_get::<Uuid>("", "alias_id")?,
            );
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_block_share_members WHERE share_id=$1",
            vec![share_id.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_block_share_audiences WHERE share_id=$1",
            vec![share_id.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_share_placements SET state='active' WHERE share_id=$1",
            vec![share_id.into()],
        ))
        .await?;
    }
    let mut members = Vec::new();
    for block in &selected {
        let logical=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT logical_id FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 AND state='active'",vec![identity.scope_id.into(),source.into(),block.physical_cid.clone().into()])).await?.ok_or_else(ApiError::internal)?.try_get::<Uuid>("","logical_id")?;
        members.push((
            old_members.get(&logical).copied().unwrap_or_else(Uuid::new_v4),
            block.physical_cid.clone(),
        ));
    }
    let hash = super::block_registry::visible_graph_hash(&prepared, &members)?;
    if existing.is_some() {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_block_shares SET publisher_user_id=$2,state='active',auth_revision=auth_revision+1,content_revision=content_revision+CASE WHEN projection_hash<>$3 THEN 1 ELSE 0 END,projection_hash=$3,source_root_cid=$4,selection=$5,source_page_id=$6,source_scope_id=$7 WHERE id=$1",vec![share_id.into(),actor.into(),hash.to_vec().into(),input.root_block_cid.clone().into(),if matches!(input.selection,Selection::Block){"block"}else{"current_subtree"}.into(),source.into(),identity.scope_id.into()])).await?;
    } else {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_shares(id,source_scope_id,source_page_id,created_by,publisher_user_id,state,projection_hash,source_root_cid,selection,root_logical_id) VALUES($1,$2,$3,$4,$4,'active',$5,$6,$7,$8)",vec![share_id.into(),identity.scope_id.into(),source.into(),actor.into(),hash.to_vec().into(),input.root_block_cid.clone().into(),if matches!(input.selection,Selection::Block){"block"}else{"current_subtree"}.into(),root_logical.into()])).await?;
    }
    for (order, (member, cid)) in members.iter().enumerate() {
        let block = selected
            .iter()
            .find(|block| block.physical_cid == *cid)
            .ok_or_else(ApiError::internal)?;
        let logical=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT logical_id FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 AND state='active'",vec![identity.scope_id.into(),source.into(),cid.clone().into()])).await?.ok_or_else(ApiError::internal)?.try_get::<Uuid>("","logical_id")?;
        let parent = block
            .parent_physical_cid
            .as_ref()
            .and_then(|parent| members.iter().find(|(_, cid)| cid == parent))
            .map(|(id, _)| *id);
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_members(share_id,member_id,logical_id,parent_member_id,member_order,content_hash) VALUES($1,$2,$3,$4,$5,$6)",vec![share_id.into(),(*member).into(),logical.into(),parent.into(),i32::try_from(order).map_err(|_|ApiError::internal())?.into(),block.content_hash.to_vec().into()])).await?;
    }
    for audience in &input.audiences {
        let target = (audience.workspace_id, audience.project_id);
        if let std::collections::btree_map::Entry::Vacant(entry) = placements.entry(target) {
            let placement = Uuid::new_v4();
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_placements(id,share_id,workspace_id,project_id,state) VALUES($1,$2,$3,$4,'active')",vec![placement.into(),share_id.into(),audience.workspace_id.into(),audience.project_id.into()])).await?;
            entry.insert(placement);
        }
        let placement = *placements.get(&target).ok_or_else(ApiError::internal)?;
        let recipients = if audience.audience == AudienceKind::Workspace {
            vec![None]
        } else {
            audience.recipient_user_ids.iter().copied().map(Some).collect()
        };
        for recipient in recipients {
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_audiences(id,share_id,workspace_id,project_id,audience_kind,recipient_user_id,permission,state,placement_id) VALUES($1,$2,$3,$4,$5,$6,$7,'active',$8)",vec![Uuid::new_v4().into(),share_id.into(),audience.workspace_id.into(),audience.project_id.into(),if recipient.is_some(){"user"}else{"workspace"}.into(),recipient.into(),if audience.permission==SharePermission::Edit{"edit"}else{"view"}.into(),placement.into()])).await?;
        }
    }
    for reference in super::body_assets::typed_references(&semantic)? {
        if let Some((member, _)) = members.iter().find(|(_, cid)| *cid == reference.block_cid) {
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_asset_aliases(share_id,member_id,alias_id,source_scope_id,source_page_id,asset_id) VALUES($1,$2,$3,$4,$5,$6)",vec![share_id.into(),(*member).into(),old_aliases.get(&(*member,reference.asset_id)).copied().unwrap_or_else(Uuid::new_v4).into(),identity.scope_id.into(),source.into(),reference.asset_id.into()])).await?;
        }
    }
    let published_revision = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT content_revision FROM flow_block_shares WHERE id=$1",
            vec![share_id.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?
        .try_get::<i64>("", "content_revision")?;
    let view = SourceView {
        root_block_cid: input.root_block_cid,
        selection: input.selection,
        share_id,
        state: "active".into(),
        revision: published_revision,
        member_count: members.len(),
        audiences: input.audiences,
    };
    let event = super::scope_event::insert(
        &tx,
        identity.scope_id,
        actor,
        if existing.is_some() {
            "flow.block_share.republished"
        } else {
            "flow.block_share.created"
        },
        "flow_object",
        source,
        key,
        serde_json::json!({"share_id":share_id}),
        serde_json::json!({"idempotency_body":body}),
        None,
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        actor,
        key,
        body,
        source,
        event,
        serde_json::to_value(&view).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}

async fn require_user_session_principal<C: ConnectionTrait>(db: &C, actor: Uuid) -> Result<(), ApiError> {
    if db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM users WHERE id=$1 AND is_active AND entity_type='human'",
            vec![actor.into()],
        ))
        .await?
        .is_none()
    {
        return Err(unavailable());
    }
    Ok(())
}

fn unavailable() -> ApiError {
    ApiError::policy_rejected_with_details(
        "shared block unavailable",
        serde_json::json!({"reason":"block_share_unavailable"}),
    )
}

fn project_content(
    node: &collab_core::SemanticNode,
    alias: Option<Uuid>,
) -> Result<collab_core::block_projection::SharedContent, ApiError> {
    use collab_core::block_projection::SharedContent as C;
    let text = node.text.clone();
    let spans = node
        .properties
        .get("inline_spans")
        .map(|raw| serde_json::from_str(raw))
        .transpose()
        .map_err(|_| unavailable())?
        .unwrap_or_default();
    let value = |key: &str| node.properties.get(key).cloned().unwrap_or_default();
    let bool_value = |key: &str| node.properties.get(key).is_some_and(|value| value == "true");
    let number = |key: &str, default: u32| {
        node.properties
            .get(key)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(default)
    };
    Ok(
        match node.properties.get("block_type").map_or("paragraph", String::as_str) {
            "paragraph" => C::Paragraph { text, spans },
            "heading" => C::Heading {
                level: u8::try_from(number("level", 1)).map_err(|_| unavailable())?,
                text,
                spans,
            },
            "bulletItem" => C::Bullet { text, spans },
            "orderedItem" => C::Ordered {
                order: number("order", 1),
                text,
                spans,
            },
            "todoItem" => C::Todo {
                checked: bool_value("checked"),
                text,
                spans,
            },
            "blockquote" => C::Quote { text, spans },
            "toggle" => C::Toggle {
                collapsed: bool_value("collapsed"),
                text,
                spans,
            },
            "codeBlock" => C::Code { text },
            "divider" => C::Divider,
            "image" => C::Image {
                asset_alias: alias.ok_or_else(unavailable)?,
                alt: value("alt"),
                caption: value("caption"),
            },
            "attachment" => C::Attachment {
                asset_alias: alias.ok_or_else(unavailable)?,
                name: value("name"),
                mime: value("mime"),
                caption: value("caption"),
            },
            "childPage" => C::ChildPlaceholder,
            _ => return Err(unavailable()),
        },
    )
}

#[derive(sea_orm::FromQueryResult)]
struct ShareRow {
    id: Uuid,
    source_scope_id: Uuid,
    source_page_id: Uuid,
    publisher_user_id: Uuid,
    state: String,
    content_revision: i64,
    auth_revision: i64,
    projection_hash: Vec<u8>,
}
async fn share_row<C: ConnectionTrait>(db: &C, id: Uuid) -> Result<ShareRow, ApiError> {
    use sea_orm::FromQueryResult;
    ShareRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,source_scope_id,source_page_id,publisher_user_id,state,content_revision,auth_revision,projection_hash FROM flow_block_shares WHERE id=$1",vec![id.into()])).one(db).await?.filter(|row|row.state=="active").ok_or_else(unavailable)
}
async fn recipient_audiences<C: ConnectionTrait>(
    db: &C,
    share: Uuid,
    actor: Uuid,
) -> Result<Vec<(Uuid, SharePermission)>, ApiError> {
    let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT DISTINCT a.workspace_id,a.permission FROM flow_block_share_audiences a JOIN flow_block_share_placements location ON location.id=a.placement_id AND location.state='active' JOIN workspace_members m ON m.workspace_id=a.workspace_id AND m.user_id=$2 JOIN users recipient ON recipient.id=m.user_id AND recipient.entity_type='human' AND recipient.is_active LEFT JOIN projects p ON p.id=a.project_id AND p.workspace_id=a.workspace_id WHERE a.share_id=$1 AND a.state='active' AND (a.audience_kind='workspace' OR a.recipient_user_id=$2) AND (a.project_id IS NULL OR p.id IS NOT NULL)",vec![share.into(),actor.into()])).await?;
    if rows.is_empty() {
        return Err(unavailable());
    }
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("", "workspace_id")?,
                if row.try_get::<String>("", "permission")? == "edit" {
                    SharePermission::Edit
                } else {
                    SharePermission::View
                },
            ))
        })
        .collect()
}

/// Independent recipient read. The complete private engine remains server-local.
pub async fn read(db: &sea_orm::DatabaseConnection, actor: Uuid, share_id: Uuid) -> Result<RecipientView, ApiError> {
    use super::{
        collab::authz::{self, PermissionLevel},
        scope,
    };
    require_user_session_principal(db, actor).await?;
    let observed = share_row(db, share_id).await?;
    require_user_session_principal(db, observed.publisher_user_id).await?;
    let audiences = recipient_audiences(db, share_id, actor).await?;
    // Continuing source sharing authority is a dependency, not an impersonated recipient grant.
    let source_identity = scope::identity(db, observed.source_page_id)
        .await
        .map_err(|_| unavailable())?;
    let source_proof = scope::authorize(
        db,
        observed.source_page_id,
        observed.publisher_user_id,
        PermissionLevel::FullAccess,
    )
    .await
    .map_err(|_| unavailable())?;
    if !source_proof.resource_active {
        return Err(unavailable());
    }
    let document = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id=$1",
            vec![observed.source_page_id.into()],
        ))
        .await?
        .ok_or_else(unavailable)?
        .try_get::<Uuid>("", "id")?;
    let engine = super::document::load_head_engine(db, document).await?;
    let semantic = engine.semantic_snapshot().map_err(|_| unavailable())?;
    let prepared = super::block_registry::prepare(&semantic)?;
    let tx = db.begin().await?;
    let workspaces = audiences
        .iter()
        .map(|(workspace, _)| *workspace)
        .chain(source_proof.audience_revisions.iter().map(|(workspace, _)| *workspace))
        .chain(source_identity.workspace_id)
        .collect::<BTreeSet<_>>();
    for workspace in &workspaces {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
            vec![(*workspace).into()],
        ))
        .await?;
        authz::lock_epoch_for_share(&tx, *workspace).await?;
    }
    scope::fence(&tx, &source_proof).await.map_err(|_| unavailable())?;
    let current_audiences = recipient_audiences(&tx, share_id, actor).await?;
    if current_audiences
        .iter()
        .any(|(workspace, _)| !workspaces.contains(workspace))
    {
        return Err(unavailable());
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_block_shares WHERE id=$1 FOR SHARE",
        vec![share_id.into()],
    ))
    .await?;
    let share = share_row(&tx, share_id).await?;
    if share.auth_revision != observed.auth_revision || share.content_revision != observed.content_revision {
        return Err(unavailable());
    }
    let head=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT d.head_frontier,d.head_seq,c.accepted_head FROM collab_documents d JOIN flow_block_checkpoints c ON c.page_id=d.object_id AND c.scope_id=$2 WHERE d.id=$1 FOR SHARE OF d,c",vec![document.into(),share.source_scope_id.into()])).await?.ok_or_else(unavailable)?;
    if !engine.matches_frontier(&head.try_get::<Vec<u8>>("", "head_frontier")?)
        || head.try_get::<i64>("", "head_seq")? != head.try_get::<i64>("", "accepted_head")?
    {
        return Err(unavailable());
    }
    let rows=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT m.member_id,p.physical_cid FROM flow_block_share_members m JOIN flow_block_placements p ON p.logical_id=m.logical_id AND p.state='active' AND p.scope_id=$2 AND p.page_id=$3 WHERE m.share_id=$1",vec![share.id.into(),share.source_scope_id.into(),share.source_page_id.into()])).await?;
    let expected = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM flow_block_share_members WHERE share_id=$1",
            vec![share.id.into()],
        ))
        .await?
        .ok_or_else(unavailable)?
        .try_get::<i64>("", "n")?;
    if i64::try_from(rows.len()).map_err(|_| unavailable())? != expected {
        return Err(unavailable());
    }
    let mut members = Vec::new();
    for row in rows {
        members.push((
            row.try_get::<Uuid>("", "member_id")?,
            row.try_get::<String>("", "physical_cid")?,
        ));
    }
    if super::block_registry::visible_graph_hash(&prepared, &members)?.as_slice() != share.projection_hash {
        return Err(unavailable());
    }
    members.sort_by_key(|(_, cid)| semantic.nodes.get(cid.as_str()).map(|node| node.order_key.clone()));
    let mut projected = Vec::new();
    for (order, (member, cid)) in members.iter().enumerate() {
        let node = semantic.nodes.get(cid.as_str()).ok_or_else(unavailable)?;
        let asset = node.properties.get("asset_id").and_then(|id| Uuid::parse_str(id).ok());
        let alias = if let Some(asset) = asset {
            tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT alias_id FROM flow_block_share_asset_aliases WHERE share_id=$1 AND member_id=$2 AND asset_id=$3",vec![share.id.into(),(*member).into(),asset.into()])).await?.map(|row|row.try_get::<Uuid>("","alias_id")).transpose()?
        } else {
            None
        };
        projected.push(collab_core::block_projection::SharedMember {
            member_id: *member,
            parent_member_id: node
                .parent
                .as_ref()
                .and_then(|parent| members.iter().find(|(_, cid)| cid == parent.as_ref()))
                .map(|(id, _)| *id),
            order: u32::try_from(order).map_err(|_| unavailable())?,
            content: project_content(node, alias)?,
        });
    }
    collab_core::block_projection::validate_members(&projected).map_err(|_| unavailable())?;
    let display_context = display_context(&tx, actor, share_id, None, None).await?;
    let result = RecipientView {
        display_context,
        projection: SharedProjection {
            share_id,
            revision: u64::try_from(share.content_revision).map_err(|_| unavailable())?,
            members: projected,
        },
        effective_permission: if current_audiences
            .iter()
            .any(|(_, permission)| *permission == SharePermission::Edit)
        {
            SharePermission::Edit
        } else {
            SharePermission::View
        },
    };
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod sql_tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    use collab_core::{CollabEngine, NodeKind, Operation};
    #[tokio::test]
    async fn source_create_receipt_and_recipient_projection_are_independent_of_private_page() {
        let scratch = scratch_or_skip!(&format!("block_share_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        db.execute_unprepared(
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
        )
        .await
        .unwrap();
        let owner = Uuid::new_v4();
        let recipient = Uuid::new_v4();
        for actor in [owner, recipient] {
            exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Share fixture','user',true)",vec![actor.into(),format!("{actor}@block-share.test").into()]).await;
        }
        let workspace = Uuid::new_v4();
        let project = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Share destination',$3)",
            vec![workspace.into(), format!("share-{workspace}").into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner'),($1,$3,'member')",
            vec![workspace.into(), owner.into(), recipient.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'SHARE','Share project',$3)",
            vec![project.into(), workspace.into(), owner.into()],
        )
        .await;
        let home = super::super::scope::personal_home(db, owner).await.unwrap();
        let created = super::super::scope::create_personal(
            db,
            owner,
            super::super::scope::CreateInput {
                editor_model: 5,
                scope_id: home.0.id,
                object_type: "page".into(),
                parent_object_id: None,
                project_id: None,
                title: "PRIVATE_PAGE_TITLE_SENTINEL".into(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                initial_fields: vec![],
                initial_view: None,
            },
        )
        .await
        .unwrap();
        let page = Uuid::parse_str(
            created
                .pointer("/object/id")
                .expect("response contains id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let document = Uuid::parse_str(
            created
                .pointer("/object/document_id")
                .expect("response contains document_id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let mut engine = super::super::document::load_head_engine(db, document).await.unwrap();
        let root = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: root.clone(),
                index: 0,
                text: "Selected 中文😀".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: "private-sibling".into(),
                parent: None,
                index: 1,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: "private-sibling".into(),
                index: 0,
                text: "PRIVATE_SIBLING_SENTINEL".into(),
            })
            .unwrap();
        let attachment_bytes = br#"{"payload":"allowed"}"#.to_vec();
        let image_bytes = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let attachment = super::super::body_assets::upload(
            db,
            page,
            owner,
            super::super::body_assets::validate_bytes(
                super::super::body_assets::AssetKind::Attachment,
                "PRIVATE_STORAGE_SENTINEL.json",
                Some("application/json"),
                attachment_bytes.clone(),
                50 * 1024 * 1024,
            )
            .unwrap(),
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
        let image = super::super::body_assets::upload(
            db,
            page,
            owner,
            super::super::body_assets::validate_bytes(
                super::super::body_assets::AssetKind::Image,
                "PRIVATE_IMAGE_SENTINEL.png",
                None,
                image_bytes.clone(),
                50 * 1024 * 1024,
            )
            .unwrap(),
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
        let attachment_cid=engine.insert_page_atom("attachment-fixture",None,2,"attachment",&serde_json::json!({"asset_id":attachment.asset_id,"name":"Allowed public.json","mime":"application/json","caption":"Visible caption","indent":0})).unwrap();
        let image_cid=engine.insert_page_atom("image-fixture",None,3,"image",&serde_json::json!({"asset_id":image.asset_id,"alt":"Allowed image","caption":"Visible image","indent":0})).unwrap();
        // Fixture setup only: install a complete initial snapshot, not an accepted-write substitute.
        exec(
            db,
            "UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$3 WHERE id=$1",
            vec![
                document.into(),
                engine.export_snapshot().unwrap().into(),
                engine.frontier().as_bytes().to_vec().into(),
            ],
        )
        .await;
        let refs = super::super::body_assets::typed_references(&engine.semantic_snapshot().unwrap()).unwrap();
        let asset_tx = db.begin().await.unwrap();
        super::super::body_assets::lock_and_sync_references(&asset_tx, home.0.id, page, 0, &refs)
            .await
            .unwrap();
        asset_tx.commit().await.unwrap();
        let private_comment = super::super::scope_social::create_comment(
            db,
            page,
            owner,
            super::super::comments::NewComment {
                body: "PRIVATE_COMMENT_SENTINEL".into(),
                block_id: Some(root.to_string()),
                parent_id: None,
                mentions: vec![],
            },
            Some(&Uuid::new_v4().to_string()),
        )
        .await
        .unwrap();
        assert_eq!(private_comment.anchor_status.as_deref(), Some("bound"));
        let key = Uuid::new_v4().to_string();
        let input = CreateInput {
            expected_frontier: base64::engine::general_purpose::STANDARD.encode(engine.frontier().as_bytes()),
            root_block_cid: root.to_string(),
            selection: Selection::Block,
            audiences: vec![AudienceInput {
                workspace_id: workspace,
                project_id: Some(project),
                audience: AudienceKind::Users,
                recipient_user_ids: vec![recipient],
                permission: SharePermission::Edit,
            }],
        };
        let view = create(db, owner, page, &key, input.clone()).await.unwrap();
        let replay = create(db, owner, page, &key, input.clone()).await.unwrap();
        assert_eq!(view.share_id, replay.share_id);
        let recipient_view = read(db, recipient, view.share_id).await.unwrap();
        assert_eq!(recipient_view.effective_permission, SharePermission::Edit);
        assert_eq!(recipient_view.projection.members.len(), 1);
        let encoded = serde_json::to_string(&recipient_view).unwrap();
        assert!(encoded.contains("Selected 中文😀"));
        for secret in [
            page.to_string(),
            document.to_string(),
            root.to_string(),
            "PRIVATE_PAGE_TITLE_SENTINEL".into(),
            "PRIVATE_SIBLING_SENTINEL".into(),
            "PRIVATE_COMMENT_SENTINEL".into(),
        ] {
            assert!(!encoded.contains(&secret));
        }
        assert!(
            super::super::scope::authorize(db, page, recipient, super::super::collab::authz::PermissionLevel::View)
                .await
                .is_err()
        );
        std::fs::write(
            "/tmp/sv-shared-projection.actual-db.fixture.json",
            serde_json::to_string_pretty(&recipient_view).unwrap(),
        )
        .unwrap();
        #[cfg(target_os = "linux")]
        verify_linux_patch(
            db,
            owner,
            recipient,
            page,
            document,
            view.share_id,
            root.to_string(),
            key.clone(),
            input.clone(),
        )
        .await;
        verify_management(
            db,
            owner,
            recipient,
            page,
            document,
            view.share_id,
            workspace,
            project,
            input,
        )
        .await;
        verify_media_alias_http(
            db,
            owner,
            recipient,
            page,
            document,
            workspace,
            attachment_cid,
            image_cid,
            attachment_bytes,
            image_bytes,
        )
        .await;
        scratch.drop_self().await;
    }

    // Pass both actors and the explicit fixture IDs used in the authorization assertions.
    #[allow(clippy::too_many_arguments)]
    async fn verify_management(
        db: &sea_orm::DatabaseConnection,
        owner: Uuid,
        recipient: Uuid,
        page: Uuid,
        document: Uuid,
        share: Uuid,
        workspace: Uuid,
        project: Uuid,
        mut input: CreateInput,
    ) {
        use base64::Engine as _;
        let body_before = super::super::document::load_head_engine(db, document)
            .await
            .unwrap()
            .semantic_snapshot()
            .unwrap()
            .semantic_hash();
        let listed = list_source(db, owner, page).await.unwrap();
        std::fs::write(
            "/tmp/sv-source-block-shares.fixture.json",
            serde_json::to_string_pretty(&listed).unwrap(),
        )
        .unwrap();
        let target_metadata = list_target_placements(db, owner, workspace, Some(project), None)
            .await
            .unwrap();
        assert_eq!(target_metadata.len(), 1);
        assert!(!target_metadata.first().unwrap().can_read);
        assert!(read(db, owner, share).await.is_err());
        assert!(discover(db, owner, workspace, Some(project)).await.unwrap().is_empty());
        let metadata_json = serde_json::to_string(&target_metadata).unwrap();
        assert!(!metadata_json.contains(&page.to_string()));
        assert!(!metadata_json.contains(&document.to_string()));
        assert!(!metadata_json.contains("PRIVATE_"));
        assert!(!metadata_json.contains("root_block_cid"));
        assert!(!metadata_json.contains("projection"));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed.first().unwrap().root_block_cid, input.root_block_cid);
        assert_eq!(
            suspend(db, owner, page, share, &Uuid::new_v4().to_string())
                .await
                .unwrap()
                .state,
            "suspended"
        );
        assert!(read(db, recipient, share).await.is_err());
        input.expected_frontier = base64::engine::general_purpose::STANDARD.encode(
            &super::super::collab::bootstrap::load(db, document)
                .await
                .unwrap()
                .head_frontier,
        );
        republish(db, owner, page, share, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        assert!(read(db, recipient, share).await.is_ok());
        // No GET between archive and restore: the durable trigger must prevent resurrection.
        exec(
            db,
            "UPDATE flow_objects SET lifecycle_status='archived',archived_at=now() WHERE id=$1",
            vec![page.into()],
        )
        .await;
        exec(
            db,
            "UPDATE flow_objects SET lifecycle_status='active',archived_at=NULL WHERE id=$1",
            vec![page.into()],
        )
        .await;
        let state = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "state")
            .unwrap();
        assert_eq!(state, "suspended");
        assert!(read(db, recipient, share).await.is_err());
        republish(db, owner, page, share, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        let scope = super::super::scope::identity(db, page).await.unwrap().scope_id;
        exec(
            db,
            "UPDATE flow_scopes SET lifecycle_status='archived' WHERE id=$1",
            vec![scope.into()],
        )
        .await;
        exec(
            db,
            "UPDATE flow_scopes SET lifecycle_status='active' WHERE id=$1",
            vec![scope.into()],
        )
        .await;
        assert!(read(db, recipient, share).await.is_err());
        republish(db, owner, page, share, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        let second = create(db, owner, page, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        let placement = read(db, recipient, share).await.unwrap().display_context.placement_id;
        remove_placement(db, owner, share, placement, &Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(read(db, recipient, share).await.is_err());
        assert!(read(db, recipient, second.share_id).await.is_ok());
        // Losing destination access cannot prevent a source controller stopping delegation.
        exec(
            db,
            "DELETE FROM workspace_members WHERE workspace_id=$1 AND user_id=$2",
            vec![workspace.into(), owner.into()],
        )
        .await;
        assert_eq!(
            suspend(db, owner, page, second.share_id, &Uuid::new_v4().to_string())
                .await
                .unwrap()
                .state,
            "suspended"
        );
        exec(
            db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
            vec![workspace.into(), owner.into()],
        )
        .await;
        republish(
            db,
            owner,
            page,
            second.share_id,
            &Uuid::new_v4().to_string(),
            input.clone(),
        )
        .await
        .unwrap();
        exec(db, "DELETE FROM projects WHERE id=$1", vec![project.into()]).await;
        let state = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_shares WHERE id=$1",
                vec![second.share_id.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "state")
            .unwrap();
        assert_eq!(state, "target_removed");
        assert!(read(db, recipient, second.share_id).await.is_err());
        assert_eq!(
            super::super::document::load_head_engine(db, document)
                .await
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .semantic_hash(),
            body_before
        );
        let target = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Second target',$3)",
            vec![target.into(), format!("share-{target}").into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner'),($1,$3,'member')",
            vec![target.into(), owner.into(), recipient.into()],
        )
        .await;
        input.audiences = vec![AudienceInput {
            workspace_id: target,
            project_id: None,
            audience: AudienceKind::Workspace,
            recipient_user_ids: vec![],
            permission: SharePermission::View,
        }];
        let third = create(db, owner, page, &Uuid::new_v4().to_string(), input)
            .await
            .unwrap();
        assert!(read(db, recipient, third.share_id).await.is_ok());
        exec(db, "DELETE FROM workspaces WHERE id=$1", vec![target.into()]).await;
        assert!(read(db, recipient, third.share_id).await.is_err());
        assert_eq!(
            super::super::document::load_head_engine(db, document)
                .await
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .semantic_hash(),
            body_before
        );
    }

    // The HTTP assertions compare both independently uploaded fixture assets.
    #[allow(clippy::too_many_arguments)]
    async fn verify_media_alias_http(
        db: &sea_orm::DatabaseConnection,
        owner: Uuid,
        recipient: Uuid,
        page: Uuid,
        document: Uuid,
        workspace: Uuid,
        attachment_cid: String,
        image_cid: String,
        attachment_bytes: Vec<u8>,
        image_bytes: Vec<u8>,
    ) {
        use axum::{
            Extension, Router,
            body::{Body, to_bytes},
            http::Request,
            routing::get,
        };
        use base64::Engine as _;
        use collab_core::block_projection::SharedContent;
        use tower::ServiceExt;
        let state = platform::app::AppState {
            db: db.clone(),
            cfg: platform::config::AppConfig {
                app_name: "share-http-unit".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("share-synthetic-test-only"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: vec![],
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        };
        let claims = platform::auth::JwtClaims {
            sub: recipient.to_string(),
            email: format!("{recipient}@block-share.test"),
            token_type: platform::auth::TokenType::Access,
            iat: 0,
            exp: usize::MAX,
            sv: 0,
        };
        // Axum resolves these path placeholders when routing requests.
        #[allow(clippy::literal_string_with_formatting_args)]
        let app = Router::new()
            .route(
                "/shares/{share}/assets/{alias}",
                get(crate::routes::flow::get_flow_v2_block_share_asset),
            )
            .layer(Extension(claims))
            .with_state(state);
        let frontier = base64::engine::general_purpose::STANDARD.encode(
            &super::super::collab::bootstrap::load(db, document)
                .await
                .unwrap()
                .head_frontier,
        );
        for (cid, expected, image) in [
            (attachment_cid, attachment_bytes, false),
            (image_cid, image_bytes, true),
        ] {
            let input = CreateInput {
                expected_frontier: frontier.clone(),
                root_block_cid: cid,
                selection: Selection::Block,
                audiences: vec![AudienceInput {
                    workspace_id: workspace,
                    project_id: None,
                    audience: AudienceKind::Users,
                    recipient_user_ids: vec![recipient],
                    permission: SharePermission::View,
                }],
            };
            let share = create(db, owner, page, &Uuid::new_v4().to_string(), input)
                .await
                .unwrap();
            let projected = read(db, recipient, share.share_id).await.unwrap();
            let (SharedContent::Attachment { asset_alias: alias, .. }
            | SharedContent::Image { asset_alias: alias, .. }) = projected.projection.members.first().unwrap().content
            else {
                panic!("expected media");
            };
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/shares/{}/assets/{alias}", share.share_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            assert_eq!(
                response.headers().get("x-flow-share-id").unwrap(),
                share.share_id.to_string().as_str()
            );
            assert_eq!(
                response.headers().get("x-flow-asset-alias").unwrap(),
                alias.to_string().as_str()
            );
            assert_eq!(response.headers().get("x-content-type-options").unwrap(), "nosniff");
            assert_eq!(response.headers().get("cache-control").unwrap(), "private, no-store");
            let disposition = response.headers().get("content-disposition").unwrap().to_str().unwrap();
            assert!(!disposition.contains("PRIVATE_"));
            if image {
                assert!(disposition.starts_with("inline;"));
                assert!(disposition.contains("shared-image.png"));
                assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
            } else {
                assert!(disposition.starts_with("attachment;"));
                assert!(disposition.contains("Allowed%20public.json"));
                assert_eq!(
                    response.headers().get("content-type").unwrap(),
                    "application/octet-stream"
                );
            }
            assert!(
                response
                    .headers()
                    .iter()
                    .all(|(_, value)| !value.to_str().unwrap_or_default().contains(&page.to_string()))
            );
            assert_eq!(
                response.headers().get("x-flow-asset-sha256").unwrap(),
                super::super::collab::bootstrap::content_hash(&expected).as_str()
            );
            assert_eq!(
                to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
                expected.as_slice()
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments, reason = "keep test actors and fixture IDs explicit")]
    async fn verify_linux_patch(
        db: &sea_orm::DatabaseConnection,
        owner: Uuid,
        recipient: Uuid,
        page: Uuid,
        document: Uuid,
        share: Uuid,
        root: String,
        create_key: String,
        create_input: CreateInput,
    ) {
        use collab_core::block_projection::{SharedCommand, SharedPatch};
        let before = read(db, recipient, share).await.unwrap();
        let member = before.projection.members.first().unwrap().member_id;
        let key = Uuid::new_v4().to_string();
        let request = SharedPatch {
            expected_revision: before.projection.revision,
            commands: vec![SharedCommand::ReplaceText {
                member_id: member,
                text: "Recipient edited 中文😀".into(),
                spans: vec![],
            }],
        };
        let saved = patch(db, recipient, share, &key, request.clone()).await.unwrap();
        assert_eq!(saved.projection.revision, before.projection.revision + 1);
        assert_eq!(
            saved.receipt.accepted_revision,
            i64::try_from(saved.projection.revision).expect("fixture revision fits i64")
        );
        let engine = super::super::document::load_head_engine(db, document).await.unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        assert_eq!(
            semantic.nodes.get(root.as_str()).unwrap().text,
            "Recipient edited 中文😀"
        );
        assert!(
            semantic
                .nodes
                .values()
                .any(|node| node.text == "PRIVATE_SIBLING_SENTINEL")
        );
        let audit = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT actor_id FROM collab_updates WHERE document_id=$1 ORDER BY seq DESC LIMIT 1",
                vec![document.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(audit.try_get::<Uuid>("", "actor_id").unwrap(), recipient);
        let head = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq FROM collab_documents WHERE id=$1",
                vec![document.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "head_seq")
            .unwrap();
        let stale = patch(db, recipient, share, &Uuid::new_v4().to_string(), request.clone())
            .await
            .unwrap_err();
        assert_eq!(stale.kind(), crate::error::ApiErrorKind::StaleFrontier);
        let unchanged = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq FROM collab_documents WHERE id=$1",
                vec![document.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "head_seq")
            .unwrap();
        assert_eq!(head, unchanged);
        let replay = patch(db, recipient, share, &key, request.clone()).await.unwrap();
        assert!(replay.receipt.replayed);
        assert_eq!(replay.receipt.accepted_revision, saved.receipt.accepted_revision);
        // A normal private sibling edit advances only the source head, never the share revision.
        let mut engine = super::super::document::load_head_engine(db, document).await.unwrap();
        let base = engine.frontier();
        let base_token = super::super::collab::bootstrap::load(db, document)
            .await
            .unwrap()
            .head_frontier;
        let semantic = engine.semantic_snapshot().unwrap();
        let (private, node) = semantic
            .nodes
            .iter()
            .find(|(_, node)| node.text == "PRIVATE_SIBLING_SENTINEL")
            .unwrap();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: private.clone(),
                index: u32::try_from(node.text.len()).expect("fixture text length fits u32"),
                text: " private activity".into(),
            })
            .unwrap();
        let bytes = engine.export_from(&base).unwrap();
        let accepted = super::super::scope_write::accept(
            db,
            page,
            bytes,
            super::super::scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "private-fixture",
                key: None,
                expected: Some(&base_token),
                command_body: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            accepted,
            super::super::collab::write::AcceptOutcome::Accepted(_)
        ));
        let confirmed = create(db, owner, page, &create_key, create_input.clone())
            .await
            .unwrap();
        assert_eq!(confirmed.share_id, share);
        let republish_key = Uuid::new_v4().to_string();
        suspend(db, owner, page, share, &Uuid::new_v4().to_string())
            .await
            .unwrap();
        let mut reviewed = create_input.clone();
        reviewed.expected_frontier = base64::engine::general_purpose::STANDARD.encode(
            &super::super::collab::bootstrap::load(db, document)
                .await
                .unwrap()
                .head_frontier,
        );
        republish(db, owner, page, share, &republish_key, reviewed.clone())
            .await
            .unwrap();
        let mut private_engine = super::super::document::load_head_engine(db, document).await.unwrap();
        let private_base = private_engine.frontier();
        let private_base_token = super::super::collab::bootstrap::load(db, document)
            .await
            .unwrap()
            .head_frontier;
        let private_semantic = private_engine.semantic_snapshot().unwrap();
        let (private, node) = private_semantic
            .nodes
            .iter()
            .find(|(_, node)| node.text.starts_with("PRIVATE_SIBLING_SENTINEL"))
            .unwrap();
        private_engine
            .apply_content_operation(&Operation::InsertText {
                id: private.clone(),
                index: u32::try_from(node.text.len()).expect("fixture text length fits u32"),
                text: " confirm replay".into(),
            })
            .unwrap();
        let accepted = super::super::scope_write::accept(
            db,
            page,
            private_engine.export_from(&private_base).unwrap(),
            super::super::scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "private-replay-fixture",
                key: None,
                expected: Some(&private_base_token),
                command_body: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            accepted,
            super::super::collab::write::AcceptOutcome::Accepted(_)
        ));
        let confirmed = republish(db, owner, page, share, &republish_key, reviewed)
            .await
            .unwrap();
        assert_eq!(confirmed.share_id, share);
        let after_private = read(db, recipient, share).await.unwrap();
        assert_eq!(
            serde_json::to_value(&after_private.projection).unwrap(),
            serde_json::to_value(&saved.projection).unwrap()
        );
        let replay = patch(db, recipient, share, &key, request).await.unwrap();
        assert!(replay.receipt.replayed);
        assert_eq!(replay.receipt.accepted_revision, saved.receipt.accepted_revision);
        assert_eq!(replay.projection.revision, saved.projection.revision);
        // This is an isolated new-module fixture, not a retry of held legacy grant scenarios.
        exec(
            db,
            "UPDATE flow_block_share_audiences SET permission='view' WHERE share_id=$1",
            vec![share.into()],
        )
        .await;
        exec(
            db,
            "UPDATE flow_block_shares SET auth_revision=auth_revision+1 WHERE id=$1",
            vec![share.into()],
        )
        .await;
        let forbidden = SharedPatch {
            expected_revision: after_private.projection.revision,
            commands: vec![SharedCommand::ReplaceText {
                member_id: member,
                text: "must not apply".into(),
                spans: vec![],
            }],
        };
        assert!(
            patch(db, recipient, share, &Uuid::new_v4().to_string(), forbidden)
                .await
                .is_err()
        );
        assert_eq!(
            super::super::document::load_head_engine(db, document)
                .await
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(root.as_str())
                .unwrap()
                .text,
            "Recipient edited 中文😀"
        );
        assert!(
            super::super::scope::authorize(
                db,
                page,
                recipient,
                super::super::collab::authz::PermissionLevel::FullAccess
            )
            .await
            .is_err()
        );
    }
}

/// Sealed server-created authorization. Raw update DTOs cannot construct this proof.
pub(crate) struct SharedWritePermit {
    share_id: Uuid,
    source: Uuid,
    actor: Uuid,
    publisher: Uuid,
    revision: i64,
    auth_revision: i64,
    before: collab_core::SemanticSnapshot,
    allowed: BTreeSet<String>,
    key: String,
    body: serde_json::Value,
}
impl SharedWritePermit {
    pub(crate) async fn authorize(
        &self,
        db: &sea_orm::DatabaseConnection,
        source: Uuid,
        actor: Uuid,
    ) -> Result<super::scope::AuthorizationProof, ApiError> {
        if self.source != source || self.actor != actor {
            return Err(unavailable());
        }
        require_user_session_principal(db, actor).await?;
        require_user_session_principal(db, self.publisher).await?;
        self.check_authority(db).await?;
        super::scope::authorize(
            db,
            source,
            self.publisher,
            super::collab::authz::PermissionLevel::FullAccess,
        )
        .await
        .map_err(|_| unavailable())
    }
    async fn check_authority<C: ConnectionTrait>(&self, db: &C) -> Result<(), ApiError> {
        let row = share_row(db, self.share_id).await?;
        if row.source_page_id != self.source
            || row.publisher_user_id != self.publisher
            || row.content_revision != self.revision
            || row.auth_revision != self.auth_revision
            || !recipient_audiences(db, self.share_id, self.actor)
                .await?
                .iter()
                .any(|(_, permission)| *permission == SharePermission::Edit)
        {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(crate) async fn fence(
        &self,
        tx: &sea_orm::DatabaseTransaction,
        proof: &super::scope::AuthorizationProof,
    ) -> Result<(), ApiError> {
        let identity = super::scope::identity(tx, self.source).await?;
        let audiences = recipient_audiences(tx, self.share_id, self.actor).await?;
        let workspaces = audiences
            .iter()
            .map(|(workspace, _)| *workspace)
            .chain(proof.audience_revisions.iter().map(|(workspace, _)| *workspace))
            .chain(identity.workspace_id)
            .collect::<BTreeSet<_>>();
        for workspace in workspaces {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
                vec![workspace.into()],
            ))
            .await?;
            super::collab::authz::lock_epoch_for_share(tx, workspace).await?;
        }
        super::scope::fence(tx, proof).await.map_err(|_| unavailable())?;
        self.check_authority(tx).await
    }
    /// Document head lock precedes share lock everywhere, including normal source reconcile.
    pub(crate) async fn lock_after_head(&self, tx: &sea_orm::DatabaseTransaction, head: i64) -> Result<(), ApiError> {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_block_shares WHERE id=$1 FOR UPDATE",
            vec![self.share_id.into()],
        ))
        .await?;
        self.check_authority(tx).await?;
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_head FROM flow_block_checkpoints WHERE page_id=$1",
                vec![self.source.into()],
            ))
            .await?
            .ok_or_else(unavailable)?;
        if row.try_get::<i64>("", "accepted_head")? != head {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(crate) fn validate_candidate(&self, candidate: &collab_core::LoroCollabEngine) -> Result<(), ApiError> {
        let after = candidate.semantic_snapshot().map_err(|_| unavailable())?;
        let diff = self.before.diff(&after);
        if !diff.added.is_empty() || !diff.removed.is_empty() {
            return Err(unavailable());
        }
        for (id, change) in diff.changed {
            if !self.allowed.contains(id.as_ref())
                || change.before.parent != change.after.parent
                || change.before.order_key != change.after.order_key
                || change.before.kind != change.after.kind
                || change.before.deleted != change.after.deleted
            {
                return Err(unavailable());
            }
            let keys = change
                .before
                .properties
                .keys()
                .chain(change.after.properties.keys())
                .collect::<BTreeSet<_>>();
            for key in keys {
                if !matches!(key.as_str(), "inline_spans" | "checked" | "collapsed")
                    && change.before.properties.get(key) != change.after.properties.get(key)
                {
                    return Err(unavailable());
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn record(&self, tx: &sea_orm::DatabaseTransaction) -> Result<(), ApiError> {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_receipts(share_id,actor_id,idempotency_key,request_body,accepted_revision) SELECT id,$2,$3,$4,content_revision FROM flow_block_shares WHERE id=$1",vec![self.share_id.into(),self.actor.into(),self.key.clone().into(),self.body.clone().into()])).await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WriteReceipt {
    pub accepted_revision: i64,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct PatchView {
    pub display_context: DisplayContext,
    pub projection: SharedProjection,
    pub effective_permission: SharePermission,
    pub receipt: WriteReceipt,
}
async fn receipt<C: ConnectionTrait>(
    db: &C,
    share: Uuid,
    actor: Uuid,
    key: &str,
    body: &serde_json::Value,
) -> Result<Option<i64>, ApiError> {
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT request_body,accepted_revision FROM flow_block_share_receipts WHERE share_id=$1 AND actor_id=$2 AND idempotency_key=$3",vec![share.into(),actor.into(),key.to_string().into()])).await?;
    if let Some(row) = row {
        if row.try_get::<serde_json::Value>("", "request_body")? != *body {
            return Err(ApiError::idempotency_key_body_drift("shared command identity changed"));
        }
        Ok(Some(row.try_get("", "accepted_revision")?))
    } else {
        Ok(None)
    }
}
fn revision_conflict(view: &RecipientView) -> ApiError {
    ApiError::typed_with_details(
        crate::error::ApiErrorKind::StaleFrontier,
        "shared block changed",
        serde_json::json!({"reason":"block_share_revision_conflict","revision":view.projection.revision,"current_projection":view.projection,"effective_permission":view.effective_permission,"write_state":"not_applied"}),
    )
}

fn shared_busy(unknown: bool) -> ApiError {
    ApiError::typed_with_details(
        crate::error::ApiErrorKind::ServerDraining(crate::error::ServerDrainingReason::Contention),
        "shared command busy; retry with the same key",
        serde_json::json!({"retry_after_ms":200,"write_state":if unknown{"unknown"}else{"not_applied"}}),
    )
}

/// Typed CAS writes are generated server-side. Recipient identity is the accepted event actor.
pub async fn patch(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    share_id: Uuid,
    key: &str,
    patch: collab_core::block_projection::SharedPatch,
) -> Result<PatchView, ApiError> {
    use collab_core::{
        CollabEngine, Operation,
        block_projection::{SharedCommand, validate_patch},
    };
    use sha2::{Digest, Sha256};
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("invalid shared command identity"));
    }
    let body = serde_json::to_value(&patch).map_err(|_| ApiError::internal())?;
    for _ in 0..3 {
        let view = read(db, actor, share_id).await?;
        if view.effective_permission != SharePermission::Edit {
            return Err(unavailable());
        }
        if let Some(accepted_revision) = receipt(db, share_id, actor, key, &body).await? {
            return Ok(PatchView {
                display_context: view.display_context,
                projection: view.projection,
                effective_permission: view.effective_permission,
                receipt: WriteReceipt {
                    accepted_revision,
                    replayed: true,
                },
            });
        }
        if patch.expected_revision != view.projection.revision {
            return Err(revision_conflict(&view));
        }
        validate_patch(&view.projection, &patch)
            .map_err(|_| ApiError::invalid_update("command outside fixed shared membership"))?;
        let share = share_row(db, share_id).await?;
        if u64::try_from(share.content_revision).map_err(|_| unavailable())? != patch.expected_revision {
            return Err(revision_conflict(&read(db, actor, share_id).await?));
        }
        let document = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM collab_documents WHERE object_id=$1",
                vec![share.source_page_id.into()],
            ))
            .await?
            .ok_or_else(unavailable)?
            .try_get::<Uuid>("", "id")?;
        let boot = super::collab::bootstrap::load(db, document).await?;
        let mut engine = super::document::engine_from_boot(&boot)?;
        let base = engine.frontier();
        let before = engine.semantic_snapshot().map_err(|_| unavailable())?;
        let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT m.member_id,p.physical_cid FROM flow_block_share_members m JOIN flow_block_placements p ON p.logical_id=m.logical_id AND p.state='active' WHERE m.share_id=$1 AND p.page_id=$2",vec![share_id.into(),share.source_page_id.into()])).await?;
        let mut members = std::collections::BTreeMap::new();
        for row in rows {
            members.insert(
                row.try_get::<Uuid>("", "member_id")?,
                row.try_get::<String>("", "physical_cid")?,
            );
        }
        let mut allowed = BTreeSet::new();
        for command in &patch.commands {
            let member_id = match command {
                SharedCommand::ReplaceText { member_id, .. }
                | SharedCommand::SetMarks { member_id, .. }
                | SharedCommand::SetChecked { member_id, .. }
                | SharedCommand::SetCollapsed { member_id, .. } => member_id,
            };
            let cid = members.get(member_id).ok_or_else(unavailable)?;
            allowed.insert(cid.clone());
            match command {
                SharedCommand::ReplaceText { text, spans, .. } => {
                    if text.chars().count() > collab_core::limits::TEXT_BLOCK_CHARS_MAX {
                        return Err(ApiError::invalid_update("shared text exceeds block limit"));
                    }
                    let current = engine.semantic_snapshot().map_err(|_| unavailable())?;
                    let node = current.nodes.get(cid.as_str()).ok_or_else(unavailable)?;
                    let old_spans: Vec<collab_core::page_inline::InlineSpan> = node
                        .properties
                        .get("inline_spans")
                        .map(|raw| serde_json::from_str(raw))
                        .transpose()
                        .map_err(|_| unavailable())?
                        .unwrap_or_default();
                    let clears = old_spans
                        .into_iter()
                        .map(|span| collab_core::page_inline::MarkEdit {
                            start: span.start,
                            end: span.end,
                            mark: span.mark,
                            clear: true,
                            href: None,
                        })
                        .collect::<Vec<_>>();
                    if !clears.is_empty() {
                        engine
                            .apply_page_marks(cid, &clears)
                            .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?;
                    }
                    let len = u32::try_from(node.text.len()).map_err(|_| unavailable())?;
                    if len > 0 {
                        engine
                            .apply_content_operation(&Operation::DeleteText {
                                id: cid.as_str().into(),
                                index: 0,
                                len,
                            })
                            .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?;
                    }
                    if !text.is_empty() {
                        engine
                            .apply_content_operation(&Operation::InsertText {
                                id: cid.as_str().into(),
                                index: 0,
                                text: text.clone(),
                            })
                            .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?;
                    }
                    if !spans.is_empty() {
                        let edits = spans
                            .iter()
                            .map(|span| collab_core::page_inline::MarkEdit {
                                start: span.start,
                                end: span.end,
                                mark: span.mark,
                                clear: false,
                                href: span.href.clone(),
                            })
                            .collect::<Vec<_>>();
                        engine
                            .apply_page_marks(cid, &edits)
                            .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?;
                    }
                }
                SharedCommand::SetMarks { edits, .. } => engine
                    .apply_page_marks(cid, edits)
                    .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?,
                SharedCommand::SetChecked { checked, .. } => engine
                    .apply_content_operation(&Operation::SetProperty {
                        id: cid.as_str().into(),
                        key: "checked".into(),
                        value: checked.to_string(),
                    })
                    .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?,
                SharedCommand::SetCollapsed { collapsed, .. } => engine
                    .apply_content_operation(&Operation::SetProperty {
                        id: cid.as_str().into(),
                        key: "collapsed".into(),
                        value: collapsed.to_string(),
                    })
                    .map_err(|_| ApiError::invalid_update("invalid shared text or mark command"))?,
            }
        }
        let permit = SharedWritePermit {
            share_id,
            source: share.source_page_id,
            actor,
            publisher: share.publisher_user_id,
            revision: share.content_revision,
            auth_revision: share.auth_revision,
            before,
            allowed,
            key: key.to_string(),
            body: body.clone(),
        };
        let bytes = engine.export_from(&base).map_err(|_| unavailable())?;
        let stable_key = format!(
            "bs:{}",
            hex::encode(Sha256::digest(format!("{share_id}:{actor}:{key}")))
        );
        let update = super::collab::write::replay_stable_update_id(document, &stable_key);
        let command_body = serde_json::json!({"operation":"block_share_patch","share_id":share_id,"patch":body});
        let outcome = super::scope_write::accept_shared(
            db,
            share.source_page_id,
            bytes,
            super::scope_write::UpdateIntent {
                user: actor,
                update,
                client: "block-share",
                key: Some(&stable_key),
                expected: Some(&boot.head_frontier),
                command_body: Some(command_body),
            },
            permit,
        )
        .await;
        if let Some(accepted_revision) = receipt(db, share_id, actor, key, &body).await? {
            let current = read(db, actor, share_id).await?;
            return Ok(PatchView {
                display_context: current.display_context,
                projection: current.projection,
                effective_permission: current.effective_permission,
                receipt: WriteReceipt {
                    accepted_revision,
                    replayed: false,
                },
            });
        }
        match outcome {
            Ok(super::collab::write::AcceptOutcome::Rejected(rejected))
                if rejected.code == super::collab::frame::RejectedCode::StaleFrontier => {}
            Err(error) => {
                let current = read(db, actor, share_id).await?;
                if current.projection.revision != patch.expected_revision {
                    return Err(revision_conflict(&current));
                }
                if error.kind() == crate::error::ApiErrorKind::StaleFrontier {
                    continue;
                }
                return Err(match error {
                    _ if error.is_database_failure() && !error.is_deterministic_database_failure() => {
                        shared_busy(false)
                    }
                    _ if matches!(
                        error.kind(),
                        crate::error::ApiErrorKind::ServerDraining(_) | crate::error::ApiErrorKind::AuthorizationChurn
                    ) =>
                    {
                        shared_busy(false)
                    }
                    _ => ApiError::server_rejected("block_share_write_refused"),
                });
            }
            Ok(super::collab::write::AcceptOutcome::Rejected(rejected))
                if rejected.recoverable || rejected.write_state == super::collab::frame::WriteState::Unknown =>
            {
                return Err(shared_busy(
                    rejected.write_state == super::collab::frame::WriteState::Unknown,
                ));
            }
            _ => {
                return Err(ApiError::policy_rejected_with_details(
                    "shared edit was not applied",
                    serde_json::json!({"reason":"block_share_write_not_applied","write_state":"not_applied"}),
                ));
            }
        }
    }
    Err(shared_busy(false))
}

fn contains_asset(view: &RecipientView, alias: Uuid) -> bool {
    view.projection.members.iter().any(|member| match member.content {
        collab_core::block_projection::SharedContent::Image { asset_alias, .. }
        | collab_core::block_projection::SharedContent::Attachment { asset_alias, .. } => asset_alias == alias,
        _ => false,
    })
}
/// Byte response vocabulary cannot carry storage IDs, paths, creator or sequence metadata.
pub struct SharedAssetBytes {
    pub share_id: Uuid,
    pub asset_alias: Uuid,
    pub content_type: String,
    pub disposition: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}
struct ShareAssetProof {
    actor: Uuid,
    share: Uuid,
    alias: Uuid,
    revision: u64,
    scope: Uuid,
    page: Uuid,
    asset: Uuid,
}
impl ShareAssetProof {
    async fn load(self, db: &sea_orm::DatabaseConnection) -> Result<SharedAssetBytes, ApiError> {
        let (asset, _) = super::body_assets::read_public_current(db, self.scope, self.page, self.asset).await?;
        let after = read(db, self.actor, self.share).await?;
        if after.projection.revision != self.revision || !contains_asset(&after, self.alias) {
            return Err(unavailable());
        }
        let content = after
            .projection
            .members
            .iter()
            .find_map(|member| match &member.content {
                collab_core::block_projection::SharedContent::Image { asset_alias, .. }
                | collab_core::block_projection::SharedContent::Attachment { asset_alias, .. }
                    if *asset_alias == self.alias =>
                {
                    Some(&member.content)
                }
                _ => None,
            })
            .ok_or_else(unavailable)?;
        let disposition = match content {
            collab_core::block_projection::SharedContent::Attachment { name, .. } => {
                let name = super::body_assets::validate_filename(name).unwrap_or_else(|_| "attachment".into());
                super::body_assets::attachment_disposition(&name)?
            }
            collab_core::block_projection::SharedContent::Image { .. } => {
                let extension = match asset.metadata.content_type.as_str() {
                    "image/png" => "png",
                    "image/jpeg" => "jpg",
                    "image/webp" => "webp",
                    "image/gif" => "gif",
                    _ => return Err(unavailable()),
                };
                format!("inline; filename=\"shared-image.{extension}\"")
            }
            _ => return Err(unavailable()),
        };
        Ok(SharedAssetBytes {
            share_id: self.share,
            asset_alias: self.alias,
            content_type: asset.metadata.content_type,
            disposition,
            sha256: asset.metadata.sha256,
            bytes: asset.bytes,
        })
    }
}
/// Alias authority is bound to a current fixed member, not merely another private Page reference.
pub async fn read_asset(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    share_id: Uuid,
    alias: Uuid,
) -> Result<SharedAssetBytes, ApiError> {
    let before = read(db, actor, share_id).await?;
    if !contains_asset(&before, alias) {
        return Err(unavailable());
    }
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT source_scope_id,source_page_id,asset_id FROM flow_block_share_asset_aliases WHERE share_id=$1 AND alias_id=$2",vec![share_id.into(),alias.into()])).await?.ok_or_else(unavailable)?;
    ShareAssetProof {
        actor,
        share: share_id,
        alias,
        revision: before.projection.revision,
        scope: row.try_get("", "source_scope_id")?,
        page: row.try_get("", "source_page_id")?,
        asset: row.try_get("", "asset_id")?,
    }
    .load(db)
    .await
}

#[derive(Clone, Debug, Serialize)]
pub struct DisplayContext {
    pub placement_id: Uuid,
    pub label: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
}
async fn display_context<C: ConnectionTrait>(
    db: &C,
    actor: Uuid,
    share: Uuid,
    workspace: Option<Uuid>,
    project: Option<Uuid>,
) -> Result<DisplayContext, ApiError> {
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT a.placement_id,a.workspace_id,w.name AS workspace_name,a.project_id,p.name AS project_name FROM flow_block_share_audiences a JOIN flow_block_share_placements location ON location.id=a.placement_id AND location.state='active' JOIN workspace_members m ON m.workspace_id=a.workspace_id AND m.user_id=$2 JOIN users recipient ON recipient.id=m.user_id AND recipient.entity_type='human' AND recipient.is_active JOIN workspaces w ON w.id=a.workspace_id LEFT JOIN projects p ON p.id=a.project_id AND p.workspace_id=a.workspace_id WHERE a.share_id=$1 AND a.state='active' AND (a.audience_kind='workspace' OR a.recipient_user_id=$2) AND (a.project_id IS NULL OR p.id IS NOT NULL) AND ($3::uuid IS NULL OR (a.workspace_id=$3 AND a.project_id IS NOT DISTINCT FROM $4::uuid)) ORDER BY a.workspace_id,a.project_id NULLS FIRST,a.id LIMIT 1",vec![share.into(),actor.into(),workspace.into(),project.into()])).await?.ok_or_else(unavailable)?;
    Ok(DisplayContext {
        placement_id: row.try_get("", "placement_id")?,
        label: "Shared block".into(),
        workspace_id: row.try_get("", "workspace_id")?,
        workspace_name: row.try_get("", "workspace_name")?,
        project_id: row.try_get("", "project_id")?,
        project_name: row.try_get("", "project_name")?,
    })
}
pub async fn read_in_context(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    share: Uuid,
    workspace: Option<Uuid>,
    project: Option<Uuid>,
) -> Result<RecipientView, ApiError> {
    let mut view = read(db, actor, share).await?;
    if workspace.is_some() {
        view.display_context = display_context(db, actor, share, workspace, project).await?;
    }
    Ok(view)
}
#[derive(Clone, Debug, Serialize)]
pub struct DiscoveryEntry {
    pub placement_id: Uuid,
    pub share_id: Uuid,
    pub revision: u64,
    pub effective_permission: SharePermission,
    pub display_context: DisplayContext,
}
pub async fn discover_page(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    workspace: Uuid,
    project: Option<Uuid>,
    cursor: Option<Uuid>,
) -> Result<Vec<DiscoveryEntry>, ApiError> {
    if db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT m.user_id FROM workspace_members m JOIN users u ON u.id=m.user_id AND u.entity_type='human' AND u.is_active WHERE m.workspace_id=$1 AND m.user_id=$2",
            vec![workspace.into(), actor.into()],
        ))
        .await?
        .is_none()
    {
        return Err(unavailable());
    }
    let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT DISTINCT a.share_id,a.project_id FROM flow_block_share_audiences a JOIN flow_block_share_placements location ON location.id=a.placement_id AND location.state='active' JOIN flow_block_shares s ON s.id=a.share_id AND s.state='active' JOIN workspace_members m ON m.workspace_id=a.workspace_id AND m.user_id=$2 JOIN users recipient ON recipient.id=m.user_id AND recipient.entity_type='human' AND recipient.is_active LEFT JOIN projects p ON p.id=a.project_id AND p.workspace_id=a.workspace_id WHERE a.workspace_id=$1 AND a.state='active' AND (a.audience_kind='workspace' OR a.recipient_user_id=$2) AND (a.project_id IS NULL OR p.id IS NOT NULL) AND ($3::uuid IS NULL OR a.project_id=$3) AND ($4::uuid IS NULL OR a.share_id>$4) ORDER BY a.share_id,a.project_id NULLS FIRST LIMIT 50",vec![workspace.into(),actor.into(),project.into(),cursor.into()])).await?;
    let mut entries = Vec::new();
    for row in rows {
        let share = row.try_get::<Uuid>("", "share_id")?;
        let target = row.try_get::<Option<Uuid>>("", "project_id")?;
        match read_in_context(db, actor, share, Some(workspace), target).await {
            Ok(view) => entries.push(DiscoveryEntry {
                placement_id: view.display_context.placement_id,
                share_id: share,
                revision: view.projection.revision,
                effective_permission: view.effective_permission,
                display_context: view.display_context,
            }),
            Err(error)
                if matches!(
                    error.kind(),
                    crate::error::ApiErrorKind::NotFound
                        | crate::error::ApiErrorKind::Forbidden
                        | crate::error::ApiErrorKind::PolicyRejected
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(entries)
}

pub async fn create(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    key: &str,
    input: CreateInput,
) -> Result<SourceView, ApiError> {
    publish(db, actor, source, key, input, None).await
}
pub async fn republish(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    share: Uuid,
    key: &str,
    input: CreateInput,
) -> Result<SourceView, ApiError> {
    publish(db, actor, source, key, input, Some(share)).await
}

async fn source_view<C: ConnectionTrait>(db: &C, share: Uuid) -> Result<SourceView, ApiError> {
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT source_root_cid,selection,state,content_revision,(SELECT count(*) FROM flow_block_share_members WHERE share_id=s.id) AS member_count FROM flow_block_shares s WHERE id=$1",vec![share.into()])).await?.ok_or_else(unavailable)?;
    let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT workspace_id,project_id,audience_kind,recipient_user_id,permission FROM flow_block_share_audiences WHERE share_id=$1 AND state='active' ORDER BY id",vec![share.into()])).await?;
    let mut audiences: Vec<AudienceInput> = Vec::new();
    for audience in rows {
        let workspace = audience.try_get::<Uuid>("", "workspace_id")?;
        let project = audience.try_get::<Option<Uuid>>("", "project_id")?;
        let permission = if audience.try_get::<String>("", "permission")? == "edit" {
            SharePermission::Edit
        } else {
            SharePermission::View
        };
        let kind = if audience.try_get::<String>("", "audience_kind")? == "workspace" {
            AudienceKind::Workspace
        } else {
            AudienceKind::Users
        };
        let index = audiences.iter().position(|candidate| {
            candidate.workspace_id == workspace
                && candidate.project_id == project
                && candidate.permission == permission
                && candidate.audience == kind
        });
        let recipient = audience.try_get::<Option<Uuid>>("", "recipient_user_id")?;
        if let Some(index) = index {
            if let Some(recipient) = recipient {
                audiences
                    .get_mut(index)
                    .ok_or_else(ApiError::internal)?
                    .recipient_user_ids
                    .push(recipient);
            }
        } else {
            audiences.push(AudienceInput {
                workspace_id: workspace,
                project_id: project,
                audience: kind,
                recipient_user_ids: recipient.into_iter().collect(),
                permission,
            });
        }
    }
    Ok(SourceView {
        root_block_cid: row.try_get("", "source_root_cid")?,
        selection: if row.try_get::<String>("", "selection")? == "block" {
            Selection::Block
        } else {
            Selection::CurrentSubtree
        },
        share_id: share,
        state: row.try_get("", "state")?,
        revision: row.try_get("", "content_revision")?,
        member_count: usize::try_from(row.try_get::<i64>("", "member_count")?).map_err(|_| ApiError::internal())?,
        audiences,
    })
}
pub async fn list_source_page(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    cursor: Option<Uuid>,
) -> Result<Vec<SourceView>, ApiError> {
    let proof = super::scope::authorize(db, source, actor, super::collab::authz::PermissionLevel::FullAccess).await?;
    let tx = db.begin().await?;
    super::scope::fence(&tx, &proof).await?;
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=$1 FOR SHARE",
        vec![source.into()],
    ))
    .await?;
    let rows = tx
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT s.id, p.physical_cid FROM flow_block_shares s
             LEFT JOIN flow_block_placements p ON p.logical_id=s.root_logical_id AND p.page_id=$1 AND p.state='active'
             WHERE (s.source_page_id=$1 OR (s.state='suspended' AND p.page_id IS NOT NULL))
               AND ($2::uuid IS NULL OR s.id>$2)
             ORDER BY s.id LIMIT 256 FOR SHARE OF s",
            vec![source.into(), cursor.into()],
        ))
        .await?;
    let mut result = Vec::new();
    for row in rows {
        let mut view = source_view(&tx, row.try_get("", "id")?).await?;
        if let Some(cid) = row.try_get::<Option<String>>("", "physical_cid")? {
            view.root_block_cid = cid;
        }
        result.push(view);
    }
    tx.commit().await?;
    Ok(result)
}

/// Stopping source delegation requires no destination access. Private target names are not read.
pub async fn suspend(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    share: Uuid,
    key: &str,
) -> Result<SourceView, ApiError> {
    stop_delegation(db, actor, source, share, key, false).await
}

pub async fn revoke(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    share: Uuid,
    key: &str,
) -> Result<SourceView, ApiError> {
    stop_delegation(db, actor, source, share, key, true).await
}

async fn stop_delegation(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    share: Uuid,
    key: &str,
    revoke: bool,
) -> Result<SourceView, ApiError> {
    let proof = super::scope::authorize(db, source, actor, super::collab::authz::PermissionLevel::FullAccess).await?;
    let identity = super::scope::identity(db, source).await?;
    let operation = if revoke {
        "block_share_revoke"
    } else {
        "block_share_suspend"
    };
    let body = serde_json::json!({"operation":operation,"source":source,"share_id":share});
    let tx = db.begin().await?;
    super::scope::fence(&tx, &proof).await?;
    let replayed = super::scope_commands::lock_and_replay(&tx, identity.scope_id, actor, key, &body)
        .await?
        .is_some();
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=$1 FOR SHARE",
        vec![source.into()],
    ))
    .await?;
    let management = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT p.physical_cid FROM flow_block_shares s
         LEFT JOIN flow_block_placements p ON p.page_id=$2 AND p.logical_id=s.root_logical_id AND p.state='active'
         WHERE s.id=$1 AND (s.source_page_id=$2 OR (
             $3 AND p.page_id IS NOT NULL AND (s.state='suspended' OR ($4 AND s.state='revoked'))
         )) FOR UPDATE OF s",
            vec![share.into(), source.into(), revoke.into(), replayed.into()],
        ))
        .await?
        .ok_or_else(unavailable)?;
    if !replayed {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_shares SET
               auth_revision=auth_revision+CASE WHEN state='active' OR ($2 AND state<>'revoked') THEN 1 ELSE 0 END,
               state=CASE WHEN $2 THEN 'revoked' WHEN state='active' THEN 'suspended' ELSE state END
             WHERE id=$1",
            vec![share.into(), revoke.into()],
        ))
        .await?;
    }
    let mut view = source_view(&tx, share).await?;
    if let Some(cid) = management.try_get::<Option<String>>("", "physical_cid")? {
        view.root_block_cid = cid;
    }
    if replayed {
        tx.commit().await?;
        return Ok(view);
    }
    let event = super::scope_event::insert(
        &tx,
        identity.scope_id,
        actor,
        if revoke {
            "flow.block_share.revoked"
        } else {
            "flow.block_share.suspended"
        },
        "flow_object",
        source,
        key,
        serde_json::json!({"share_id":share}),
        serde_json::json!({"idempotency_body":body}),
        None,
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        actor,
        key,
        body,
        source,
        event,
        serde_json::to_value(&view).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}

#[derive(Clone, Debug, Serialize)]
pub struct RemovedPlacement {
    pub share_id: Uuid,
    pub placement_id: Uuid,
    pub state: String,
}
/// Destination admins can remove this binding, never acquire source Page authority.
pub async fn remove_placement(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    share: Uuid,
    placement: Uuid,
    key: &str,
) -> Result<RemovedPlacement, ApiError> {
    let target = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id FROM flow_block_share_placements WHERE id=$1 AND share_id=$2",
            vec![placement.into(), share.into()],
        ))
        .await?
        .ok_or_else(unavailable)?
        .try_get::<Uuid>("", "workspace_id")?;
    let body =
        serde_json::json!({"operation":"block_share_remove_placement","share_id":share,"placement_id":placement});
    let tx = db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
        vec![target.into()],
    ))
    .await?;
    super::collab::authz::lock_epoch_for_share(&tx, target).await?;
    if !super::scope_mounts::destination_can_manage(&tx, target, actor).await? {
        return Err(unavailable());
    }
    let result = RemovedPlacement {
        share_id: share,
        placement_id: placement,
        state: "target_removed".into(),
    };
    if receipt(&tx, share, actor, key, &body).await?.is_some() {
        tx.commit().await?;
        return Ok(result);
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_block_shares WHERE id=$1 FOR UPDATE",
        vec![share.into()],
    ))
    .await?;
    // Recheck the exact binding after the share lock: republish cannot retarget this share.
    let changed = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_share_placements SET state='removed' WHERE id=$1 AND share_id=$2 AND workspace_id=$3",
            vec![placement.into(), share.into(), target.into()],
        ))
        .await?;
    if changed.rows_affected() != 1 {
        return Err(unavailable());
    }
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_block_shares SET state='target_removed',auth_revision=auth_revision+1 WHERE id=$1 AND state<>'revoked'",vec![share.into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_receipts(share_id,actor_id,idempotency_key,request_body,accepted_revision) SELECT id,$2,$3,$4,content_revision FROM flow_block_shares WHERE id=$1",vec![share.into(),actor.into(),key.to_string().into(),body.into()])).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn list_source(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    source: Uuid,
) -> Result<Vec<SourceView>, ApiError> {
    list_source_page(db, actor, source, None).await
}
pub async fn discover(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    workspace: Uuid,
    project: Option<Uuid>,
) -> Result<Vec<DiscoveryEntry>, ApiError> {
    discover_page(db, actor, workspace, project, None).await
}
#[derive(Clone, Debug, Serialize)]
pub struct TargetPlacementView {
    pub share_id: Uuid,
    pub placement_id: Uuid,
    pub state: String,
    pub can_read: bool,
    pub display_context: DisplayContext,
}
/// Target management discloses binding metadata only, never promotes administrators to readers.
pub async fn list_target_placements(
    db: &sea_orm::DatabaseConnection,
    actor: Uuid,
    workspace: Uuid,
    project: Option<Uuid>,
    cursor: Option<Uuid>,
) -> Result<Vec<TargetPlacementView>, ApiError> {
    require_user_session_principal(db, actor).await?;
    let tx = db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
        vec![workspace.into()],
    ))
    .await?;
    super::collab::authz::lock_epoch_for_share(&tx, workspace).await?;
    if !super::scope_mounts::destination_can_manage(&tx, workspace, actor).await? {
        return Err(unavailable());
    }
    let rows=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.id,p.share_id,p.workspace_id,w.name AS workspace_name,p.project_id,target.name AS project_name,s.state FROM flow_block_share_placements p JOIN flow_block_shares s ON s.id=p.share_id JOIN workspaces w ON w.id=p.workspace_id LEFT JOIN projects target ON target.id=p.project_id AND target.workspace_id=p.workspace_id WHERE p.workspace_id=$1 AND p.state='active' AND s.state<>'revoked' AND (p.project_id IS NULL OR target.id IS NOT NULL) AND ($2::uuid IS NULL OR p.project_id=$2) AND ($3::uuid IS NULL OR p.id>$3) ORDER BY p.id LIMIT 50",vec![workspace.into(),project.into(),cursor.into()])).await?;
    let mut result = Vec::new();
    for row in rows {
        let id = row.try_get::<Uuid>("", "id")?;
        result.push(TargetPlacementView {
            share_id: row.try_get("", "share_id")?,
            placement_id: id,
            state: row.try_get("", "state")?,
            can_read: false,
            display_context: DisplayContext {
                placement_id: id,
                label: "Shared block".into(),
                workspace_id: workspace,
                workspace_name: row.try_get("", "workspace_name")?,
                project_id: row.try_get("", "project_id")?,
                project_name: row.try_get("", "project_name")?,
            },
        });
    }
    tx.commit().await?;
    for item in &mut result {
        item.can_read = read(db, actor, item.share_id).await.is_ok();
    }
    Ok(result)
}
