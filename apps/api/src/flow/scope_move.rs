//! Same-owner-scope personal reparenting. SQL parent and navigator ordering commit together.
//! CRDT preparation stays outside locks and uses the shared isolated-apply boundary.
use super::json_value::FlowJson;
use super::{
    collab::{authz::PermissionLevel, bootstrap, runtime, write},
    scope,
};
use crate::error::{ApiError, ApiErrorKind};
use base64::{Engine, engine::general_purpose::STANDARD};
use collab_core::{CollabEngine, LoroCollabEngine};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MovePayload {
    target_object_id: Uuid,
    #[serde(default)]
    after_id: Option<Uuid>,
    #[serde(default)]
    expected_target_frontier: Option<String>,
    #[serde(default)]
    confirm_self_lockout: bool,
}
#[derive(FromQueryResult)]
struct Doc {
    id: Uuid,
}
#[derive(FromQueryResult)]
struct Head {
    head_seq: i64,
}

fn validate_kinds(source: &scope::ObjectIdentity, target: &scope::ObjectIdentity) -> Result<(), ApiError> {
    if source.scope_id != target.scope_id || source.workspace_id.is_some() || target.workspace_id.is_some() {
        return Err(ApiError::not_found("target_object_id not found"));
    }
    if source.project_id.is_some() || target.project_id.is_some() {
        return Err(ApiError::policy_rejected(
            "personal objects cannot carry project ownership",
        ));
    }
    if source.lifecycle_status != "active" || target.lifecycle_status != "active" {
        return Err(ApiError::invalid_update(
            "archived objects cannot be moved or adopt children",
        ));
    }
    if source.parent_id.is_none() || !matches!(source.object_type.as_str(), "page" | "collection") {
        return Err(ApiError::invalid_update(
            "only non-root Pages and Collections can be moved; Records retain their Collection association",
        ));
    }
    if !matches!(target.object_type.as_str(), "page" | "navigator") {
        return Err(ApiError::invalid_update("target parent must be a Page or navigator"));
    }
    if source.id == target.id {
        return Err(ApiError::invalid_update("an object cannot be moved under itself"));
    }
    Ok(())
}

/// Validate every existing descendant, including archived rows; never repair corrupt ownership.
pub(crate) async fn validate_structure<C: ConnectionTrait>(
    db: &C,
    source: &scope::ObjectIdentity,
    target: &scope::ObjectIdentity,
) -> Result<(), ApiError> {
    validate_kinds(source, target)?;
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
WITH RECURSIVE ancestors AS (
 SELECT id,parent_id,scope_id,project_id,0 AS depth,ARRAY[id] AS path,false AS cycle FROM flow_objects WHERE id=$2
 UNION ALL SELECT o.id,o.parent_id,o.scope_id,o.project_id,a.depth+1,a.path||o.id,o.id=ANY(a.path)
 FROM flow_objects o JOIN ancestors a ON o.id=a.parent_id WHERE a.depth<33 AND NOT a.cycle
), descendants AS (
 SELECT id,parent_id,scope_id,project_id,0 AS depth,ARRAY[id] AS path,false AS cycle FROM flow_objects WHERE id=$1
 UNION ALL SELECT o.id,o.parent_id,o.scope_id,o.project_id,d.depth+1,d.path||o.id,o.id=ANY(d.path)
 FROM flow_objects o JOIN descendants d ON o.parent_id=d.id WHERE d.depth<33 AND NOT d.cycle
)
SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=$1 OR cycle OR scope_id<>$3 OR project_id IS NOT NULL) OR
 EXISTS(SELECT 1 FROM descendants WHERE cycle OR scope_id<>$3 OR project_id IS NOT NULL) AS invalid,
 (SELECT max(depth) FROM ancestors)+(SELECT max(depth) FROM descendants)+1 AS depth",
            vec![source.id.into(), target.id.into(), source.scope_id.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if row.try_get::<bool>("", "invalid")? || row.try_get::<i32>("", "depth")? > 32 {
        return Err(ApiError::invalid_update(
            "move would create a cycle, cross ownership, or exceed tree depth32",
        ));
    }
    Ok(())
}

pub async fn navigator_positions(
    db: &DatabaseConnection,
    root: Uuid,
    expected_seq: i64,
) -> Result<std::collections::HashMap<Uuid, String>, ApiError> {
    let document = Doc::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=$1",
        vec![root.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)?;
    let boot = bootstrap::load(db, document.id).await?;
    if boot.head_seq != expected_seq {
        return Err(ApiError::stale_frontier(
            "navigator changed during read",
            Some(boot.head_seq),
            None,
        ));
    }
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|e| super::command::map_collab_error(&e))?;
    for update in &boot.tail_updates {
        engine
            .import_update(&update.bytes)
            .map_err(|e| super::command::map_collab_error(&e))?;
    }
    let identity = scope::identity(db, root).await?;
    let rows=Doc::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM flow_objects WHERE scope_id=$1 AND project_id IS NOT DISTINCT FROM $2 AND parent_id IS NOT NULL AND lifecycle_status='active' ORDER BY created_at,id LIMIT 1001",vec![identity.scope_id.into(),identity.project_id.into()])).all(db).await?;
    if u64::try_from(rows.len()).unwrap_or(u64::MAX) > super::collab::limits::AUTHORIZED_SCAN_ROWS_MAX {
        return Err(ApiError::limit_exceeded(
            "navigator scan budget exceeded",
            "scan_budget",
            Some(json!(super::collab::limits::AUTHORIZED_SCAN_ROWS_MAX)),
            None,
            None,
        ));
    }
    let ids: Vec<_> = rows.into_iter().map(|row| row.id).collect();
    let mut order = engine.navigator_order();
    seed_missing_keys(&mut order, &ids)?;
    Ok(ids
        .into_iter()
        .filter_map(|id| order.get(&id.to_string()).cloned().map(|key| (id, key)))
        .collect())
}

const ORDER_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
fn fractional_between(lower: Option<&str>, upper: Option<&str>) -> Result<String, ApiError> {
    let lo = lower.unwrap_or("");
    let hi = upper.unwrap_or("");
    if (!hi.is_empty() && lo >= hi)
        || [lo, hi]
            .iter()
            .any(|key| key.len() > 65 || !key.bytes().all(|b| ORDER_ALPHABET.contains(&b)))
    {
        return Err(ApiError::invalid_update("invalid navigator order bounds"));
    }
    let mut result = String::new();
    for index in 0..=64 {
        let low = lo
            .as_bytes()
            .get(index)
            .and_then(|c| ORDER_ALPHABET.iter().position(|b| b == c))
            .unwrap_or(0);
        let high = hi
            .as_bytes()
            .get(index)
            .and_then(|c| ORDER_ALPHABET.iter().position(|b| b == c))
            .unwrap_or(36);
        if high > low + 1 {
            result.push(char::from(
                *ORDER_ALPHABET
                    .get(low + (high - low) / 2)
                    .ok_or_else(ApiError::internal)?,
            ));
            if result.as_str() > lo && (hi.is_empty() || result.as_str() < hi) {
                return Ok(result);
            }
            break;
        }
        result.push(char::from(*ORDER_ALPHABET.get(low).ok_or_else(ApiError::internal)?));
    }
    Err(ApiError::invalid_update(
        "navigator order key depth or bounds exhausted",
    ))
}
fn seed_missing_keys(order: &mut std::collections::BTreeMap<String, String>, ordered: &[Uuid]) -> Result<(), ApiError> {
    let mut last = order.values().max().cloned();
    for id in ordered {
        if let std::collections::btree_map::Entry::Vacant(entry) = order.entry(id.to_string()) {
            let key = fractional_between(last.as_deref(), None)?;
            entry.insert(key.clone());
            last = Some(key);
        }
    }
    Ok(())
}
fn prepare_order_update(
    engine: &mut LoroCollabEngine,
    ordered: &[Uuid],
    siblings: &[Uuid],
    object: Uuid,
    after: Option<Uuid>,
) -> Result<Option<Vec<u8>>, ApiError> {
    let base = engine.frontier();
    let before = engine.navigator_order();
    let mut order = before.clone();
    seed_missing_keys(&mut order, ordered)?;
    let mut siblings: Vec<_> = siblings.iter().copied().filter(|id| *id != object).collect();
    siblings.sort_by(|a, b| order.get(&a.to_string()).cmp(&order.get(&b.to_string())).then(a.cmp(b)));
    let insertion = after
        .and_then(|id| siblings.iter().position(|s| *s == id))
        .map_or(siblings.len(), |i| i + 1);
    let lower = insertion
        .checked_sub(1)
        .and_then(|i| siblings.get(i))
        .and_then(|id| order.get(&id.to_string()));
    let upper = siblings.get(insertion).and_then(|id| order.get(&id.to_string()));
    let key = fractional_between(lower.map(String::as_str), upper.map(String::as_str))?;
    order.insert(object.to_string(), key);
    // All bounds are validated before touching the private candidate. Tree entries mirror placement;
    // the existing browser order map is authoritative, never replaced or renumbered.
    super::move_object::complete_and_reorder_scope_navigator(engine, ordered, object, after)?;
    for (id, key) in &order {
        if before.get(id) != Some(key) {
            engine
                .set_navigator_order(id, key)
                .map_err(|e| super::command::map_collab_error(&e))?;
        }
    }
    if engine.frontier().as_bytes() == base.as_bytes() {
        return Ok(None);
    }
    let bytes = engine
        .export_from(&base)
        .map_err(|e| super::command::map_collab_error(&e))?;
    Ok((!bytes.is_empty()).then_some(bytes))
}

async fn ordered_objects<C: ConnectionTrait>(db: &C, scope: Uuid) -> Result<Vec<Uuid>, ApiError> {
    let rows=Doc::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM flow_objects WHERE scope_id=$1 AND parent_id IS NOT NULL AND lifecycle_status='active' ORDER BY created_at,id LIMIT 1001",vec![scope.into()])).all(db).await?;
    if u64::try_from(rows.len()).unwrap_or(u64::MAX) > super::collab::limits::AUTHORIZED_SCAN_ROWS_MAX {
        return Err(ApiError::limit_exceeded(
            "navigator ordering scan budget exceeded",
            "scan_budget",
            Some(json!(super::collab::limits::AUTHORIZED_SCAN_ROWS_MAX)),
            None,
            None,
        ));
    }
    Ok(rows.into_iter().map(|r| r.id).collect())
}

pub async fn execute(
    state: &AppState,
    object: Uuid,
    user: Uuid,
    payload: Value,
    key: &str,
    expected: Option<&str>,
) -> Result<Value, ApiError> {
    if expected.is_some() {
        return Err(ApiError::invalid_update(
            "move_object advances navigator ordering, not source content; use expected_target_frontier",
        ));
    }
    let input: MovePayload =
        serde_json::from_value(payload.clone()).map_err(|_| ApiError::invalid_update("invalid move_object payload"))?;
    let body = json!({"operation":"move_object","object_id":object,"payload":payload});
    let initial = scope::identity(&state.db, object).await?;
    scope::authorize(&state.db, object, user, PermissionLevel::FullAccess).await?;
    if initial.workspace_id.is_some() {
        return Err(ApiError::invalid_update("legacy moves require their workspace adapter"));
    }
    if let Some(result) = super::scope_commands::replay(&state.db, initial.scope_id, user, key, &body).await? {
        return Ok(result);
    }
    // Authenticate the destination before structural diagnostics can reveal its kind.
    scope::authorize(&state.db, input.target_object_id, user, PermissionLevel::Edit).await?;
    let target = scope::identity(&state.db, input.target_object_id).await?;
    validate_structure(&state.db, &initial, &target).await?;
    let root = scope::roots(&state.db, initial.scope_id)
        .await?
        .into_iter()
        .find(|r| r.project_id.is_none())
        .ok_or_else(ApiError::internal)?
        .object_id;
    let document = Doc::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=$1",
        vec![root.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(ApiError::internal)?
    .id;
    let _coordinator = runtime::runtime()
        .coordinator
        .acquire(document)
        .await
        .map_err(|_| ApiError::policy_rejected("navigator busy; retry the same intent"))?;
    let expected = input
        .expected_target_frontier
        .as_deref()
        .map(|s| {
            STANDARD
                .decode(s)
                .map_err(|_| ApiError::invalid_update("invalid target frontier"))
        })
        .transpose()?;
    for _ in 0..3 {
        let source = scope::identity(&state.db, object).await?;
        let mut proof = scope::authorize(&state.db, object, user, PermissionLevel::FullAccess).await?;
        let destination = scope::authorize(&state.db, input.target_object_id, user, PermissionLevel::Edit).await?;
        proof.audience_revisions.extend(destination.audience_revisions);
        if let Some(after) = input.after_id {
            let sibling = scope::identity(&state.db, after).await?;
            let predecessor = scope::authorize(&state.db, after, user, PermissionLevel::View).await?;
            proof.audience_revisions.extend(predecessor.audience_revisions);
            if sibling.scope_id != source.scope_id
                || sibling.parent_id != Some(input.target_object_id)
                || after == object
                || sibling.lifecycle_status != "active"
            {
                return Err(ApiError::invalid_update(
                    "after_id must be an active destination sibling",
                ));
            }
        }
        proof.audience_revisions.sort_unstable();
        proof.audience_revisions.dedup();
        let boot = bootstrap::load(&state.db, document).await?;
        if expected.as_ref().is_some_and(|f| f != &boot.head_frontier) {
            return Err(ApiError::stale_frontier("target navigator changed", None, None));
        }
        let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|e| super::command::map_collab_error(&e))?;
        for tail in &boot.tail_updates {
            engine
                .import_update(&tail.bytes)
                .map_err(|e| super::command::map_collab_error(&e))?;
        }
        let ordered = ordered_objects(&state.db, source.scope_id).await?;
        let sibling_rows=Doc::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM flow_objects WHERE scope_id=$1 AND parent_id=$2 AND lifecycle_status='active' ORDER BY created_at,id LIMIT 1001",vec![source.scope_id.into(),input.target_object_id.into()])).all(&state.db).await?;
        let siblings: Vec<_> = sibling_rows.into_iter().map(|r| r.id).collect();
        let bytes = prepare_order_update(&mut engine, &ordered, &siblings, object, input.after_id)?;
        let update = write::replay_stable_update_id(document, key);
        let prepared = if let Some(bytes) = bytes.as_ref() {
            match write::hydrate_and_apply(
                &state.db,
                &runtime::runtime().cache,
                document,
                update,
                bytes,
                expected.as_deref(),
            )
            .await?
            {
                write::HydrateOutcome::Prepared(p) => Some(p),
                write::HydrateOutcome::Rejected(write::AcceptOutcome::Rejected(r)) => {
                    return Err(move_rejection(r));
                }
                write::HydrateOutcome::Rejected(write::AcceptOutcome::Accepted(_)) => return Err(ApiError::internal()),
            }
        } else {
            None
        };
        let tx = state.db.begin().await?;
        write::set_locked_phase_statement_budgets(&tx, 1).await?;
        scope::require_personal_write(&tx).await?;
        if scope::get(&tx, source.scope_id).await?.lifecycle_status != "active" {
            return Err(ApiError::policy_rejected("scope is inactive"));
        }
        scope::lock_change(&tx, &proof).await?;
        let enabled = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT writes_enabled FROM flow_scope_settings WHERE scope_id=$1",
                vec![source.scope_id.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        if !enabled.try_get::<bool>("", "writes_enabled")? {
            return Err(ApiError::policy_rejected("scope writes are disabled"));
        }
        if let Some(prior) = super::scope_commands::lock_and_replay(&tx, source.scope_id, user, key, &body).await? {
            tx.commit().await?;
            return Ok(prior);
        }
        // Exclusive scope authority serializes personal graph mutations and fences child creation.
        // Lock only source and target ancestry in ascending ID order, not the whole scope.
        tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"WITH RECURSIVE chain AS (SELECT id,parent_id,0 AS depth FROM flow_objects WHERE id=$2 UNION ALL SELECT o.id,o.parent_id,c.depth+1 FROM flow_objects o JOIN chain c ON o.id=c.parent_id WHERE c.depth<33) SELECT id FROM flow_objects WHERE id=$1 OR id IN (SELECT id FROM chain) ORDER BY id FOR UPDATE",vec![object.into(),input.target_object_id.into()])).await?;
        if ordered_objects(&tx, source.scope_id).await? != ordered {
            tx.rollback().await?;
            continue;
        }
        let current = scope::identity(&tx, object).await?;
        let target = scope::identity(&tx, input.target_object_id).await?;
        validate_structure(&tx, &current, &target).await?;
        scope::authorize(&tx, object, user, PermissionLevel::FullAccess).await?;
        scope::authorize(&tx, target.id, user, PermissionLevel::Edit).await?;
        if let Some(after) = input.after_id {
            let row = scope::identity(&tx, after).await?;
            scope::authorize(&tx, after, user, PermissionLevel::View).await?;
            if row.parent_id != Some(target.id) || row.scope_id != source.scope_id || row.lifecycle_status != "active" {
                return Err(ApiError::invalid_update("after_id changed"));
            }
        }
        let head = Head::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq FROM collab_documents WHERE id=$1 FOR UPDATE",
            vec![document.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(ApiError::internal)?;
        if head.head_seq != boot.head_seq {
            tx.rollback().await?;
            continue;
        }
        write::set_locked_phase_statement_budgets(&tx, 1).await?;
        let before_view = scope::object_view(&tx, object, user).await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET parent_id=$2,updated_at=now(),updated_by=$3 WHERE id=$1",
            vec![object.into(), target.id.into(), user.into()],
        ))
        .await?;
        let after_level = match scope::authorize(&tx, object, user, PermissionLevel::FullAccess).await {
            Ok(value) => Some(value),
            Err(error) if error.kind() == ApiErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if after_level.is_none() && !input.confirm_self_lockout {
            return Err(ApiError::policy_rejected_with_details(
                "move would reduce your access; confirmation required",
                json!({"reason":"self_lockout_confirmation_required"}),
            ));
        }
        if let Some(p) = prepared.as_ref() {
            stage(
                &tx,
                NavigatorUpdate {
                    scope: source.scope_id,
                    root,
                    document,
                    user,
                    key,
                    update,
                    bytes: bytes.as_deref().ok_or_else(ApiError::internal)?,
                    prepared: p,
                },
            )
            .await?;
        }
        let revision = scope::advance_revision(&tx, source.scope_id).await?;
        let event = super::scope_event::insert_for_object(
            &tx,
            &source,
            user,
            "flow.object.moved",
            "flow_object",
            object,
            key,
            json!({"object_id":object,"target_object_id":target.id}),
            json!({}),
        )
        .await?;
        let view = match scope::object_view(&tx, object, user).await {
            Ok(value) => Some(value),
            Err(error) if error.kind() == ApiErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let root_readable = match scope::authorize(&tx, root, user, PermissionLevel::View).await {
            Ok(_) => true,
            Err(e) if e.kind() == ApiErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        let mut result = scope::accepted(view.clone().unwrap_or(before_view), event);
        if view.is_none() {
            *result.field_mut("object")? = Value::Null;
        }
        let mut affected = vec![object, target.id];
        if root_readable {
            affected.push(root);
        }
        affected.sort_unstable();
        affected.dedup();
        *result.field_mut("affected_object_ids")? = json!(affected);
        *result.field_mut("command_result")? = json!({"command":"move_object","object_id":object,"parent_id":target.id,"existing_document_cardinality":"bounded_many","existing_document_cardinality_max":1,"committed_authz_revision":revision});
        if root_readable {
            *result.field_mut("command_result")?.field_mut("navigator_document_id")? = json!(document);
            *result.field_mut("command_result")?.field_mut("navigator_seq")? =
                json!(head.head_seq + i64::from(prepared.is_some()));
        }

        super::scope_commands::record(
            &tx,
            source.scope_id,
            user,
            key,
            body.clone(),
            object,
            event,
            result.clone(),
        )
        .await?;
        tx.commit().await?;
        if let Some(p) = prepared.as_ref()
            && let Ok(engine) = p.candidate.fork()
        {
            runtime::runtime().cache.put(
                document,
                engine,
                p.observed.format_version.clone(),
                head.head_seq + 1,
                p.after_frontier.clone(),
                p.decoded_bytes_hint,
            );
        }
        super::collab::snapshot::spawn_background(&runtime::runtime().snapshot, state.db.clone(), document);
        return Ok(result);
    }
    Err(ApiError::policy_rejected(
        "navigator changed repeatedly; retry the same intent",
    ))
}
// Moving an object does not grant read access to the private navigator document. Keep safe
// rejection classification while withholding its identity/frontier/sequence even from stale fences.
fn move_rejection(mut rejected: write::Rejected) -> ApiError {
    rejected.current_seq = None;
    rejected.current_frontier = None;
    if let Some(details) = rejected.details.as_mut().and_then(Value::as_object_mut) {
        for key in [
            "document_id",
            "object_id",
            "current_seq",
            "current_frontier",
            "minimum_snapshot_seq",
            "head_seq",
            "head_frontier",
            "snapshot_seq",
        ] {
            details.remove(key);
        }
    }
    super::command::map_write_rejection(&rejected)
}

struct NavigatorUpdate<'a> {
    scope: Uuid,
    root: Uuid,
    document: Uuid,
    user: Uuid,
    key: &'a str,
    update: Uuid,
    bytes: &'a [u8],
    prepared: &'a write::Prepared,
}
async fn stage(tx: &sea_orm::DatabaseTransaction, input: NavigatorUpdate<'_>) -> Result<(), ApiError> {
    let NavigatorUpdate {
        scope,
        root,
        document,
        user,
        key,
        update,
        bytes,
        prepared: p,
    } = input;
    let seq = p.observed.head_seq + 1;
    let event = super::scope_event::insert(
        tx,
        scope,
        user,
        "flow.content.accepted",
        "flow_document",
        document,
        &format!("move-content:{document}:{key}"),
        json!({"object_id":root,"document_id":document,"accepted_seq":seq,"projection_seq":seq,"changed_block_ids":[]}),
        json!({"semantic_summary":{"action":"navigator_reorder"}}),
        Some((document, seq)),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO collab_updates(document_id,seq,update_id,content_hash,idempotency_key,before_frontier,after_frontier,bytes,actor_id,origin_surface,origin_client_id,projection_seq,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'rest','scope-move',$2,$10)",vec![document.into(),seq.into(),update.into(),p.content_hash_hex.clone().into(),key.into(),p.observed.head_frontier.clone().into(),p.after_frontier.clone().into(),bytes.to_vec().into(),user.into(),event.into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET head_seq=$2,head_frontier=$3,byte_count=byte_count+$4,update_count=update_count+1,updated_at=now() WHERE id=$1",vec![document.into(),seq.into(),p.after_frontier.clone().into(),i64::try_from(bytes.len()).unwrap_or(i64::MAX).into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_object_projections SET document_seq=$2,document_frontier=$3,title=$4,state=$5,plain_text=$6,updated_at=now() WHERE object_id=$1",vec![root.into(),seq.into(),p.after_frontier.clone().into(),p.title.clone().into(),p.state_json.clone().into(),p.plain_text.clone().into()])).await?;
    Ok(())
}

/// Unplaced newly-created objects follow accepted entries in query creation order, never UUID order.
pub fn position_for<S: std::hash::BuildHasher>(
    positions: &std::collections::HashMap<Uuid, String, S>,
    id: Uuid,
    fallback_index: usize,
) -> Result<String, ApiError> {
    if let Some(key) = positions.get(&id) {
        return Ok(key.clone());
    }
    let max = positions.values().max().map_or("", String::as_str);
    let key = format!("{max}i{fallback_index:08x}");
    if key.len() > 65 {
        return Err(ApiError::invalid_update("navigator fallback order key depth exceeded"));
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use collab_core::{NodeId, NodeKind, Operation};
    use std::sync::Arc;
    fn identity(id: Uuid, kind: &str, parent: Option<Uuid>) -> scope::ObjectIdentity {
        scope::ObjectIdentity {
            id,
            scope_id: Uuid::nil(),
            workspace_id: None,
            project_id: None,
            parent_id: parent,
            object_type: kind.into(),
            lifecycle_status: "active".into(),
        }
    }
    #[test]
    fn typed_and_scope_structure_is_not_single_row_reparented() {
        let parent = Uuid::new_v4();
        let page = identity(Uuid::new_v4(), "page", Some(parent));
        let target = identity(parent, "page", Some(Uuid::new_v4()));
        assert!(validate_kinds(&page, &target).is_ok());
        let mut record = page.clone();
        record.object_type = "record".into();
        assert!(validate_kinds(&record, &target).is_err());
        let mut collection = target.clone();
        collection.object_type = "collection".into();
        assert!(validate_kinds(&page, &collection).is_err());
        let mut foreign = target.clone();
        foreign.scope_id = Uuid::new_v4();
        assert!(validate_kinds(&page, &foreign).is_err());
        let root = identity(Uuid::new_v4(), "navigator", None);
        assert!(validate_kinds(&root, &target).is_err());
    }
    #[test]
    fn baseline_reorder_preserves_nodes_and_missing_entries_append() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let mut engine = LoroCollabEngine::new_empty(23);
        for (index, id) in [a, b].into_iter().enumerate() {
            engine
                .apply_operation(&Operation::CreateNode {
                    id: Arc::from(id.to_string()) as NodeId,
                    parent: None,
                    index: u32::try_from(index).unwrap(),
                    kind: NodeKind::NavigatorNode,
                })
                .unwrap();
        }
        super::super::move_object::reorder_scope_navigator(&mut engine, c, Some(a)).unwrap();
        let positions = super::super::move_object::navigator_positions(&engine.semantic_snapshot().unwrap());
        assert!(positions.get(&a).unwrap() < positions.get(&c).unwrap());
        assert!(positions.get(&c).unwrap() < positions.get(&b).unwrap());
        assert_eq!(positions.len(), 3);
        let mut blank = LoroCollabEngine::new_empty(75);
        prepare_order_update(&mut blank, &[a, b, c], &[a, b, c], a, Some(b)).unwrap();
        let order = blank.navigator_order();
        assert!(order.get(&b.to_string()).unwrap() < order.get(&a.to_string()).unwrap());
        assert!(order.get(&a.to_string()).unwrap() < order.get(&c.to_string()).unwrap());
        let ranked = std::collections::HashMap::from([(a, "i".into())]);
        assert_eq!(position_for(&ranked, a, 500).unwrap(), "i");
    }
    #[test]
    fn browser_map_only_snapshot_server_move_preserves_container_and_unmoved_keys() {
        let a = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let b = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        let c = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        let mut engine = LoroCollabEngine::load(include_bytes!(
            "../../../../frontend/tests/fixtures/flow-navigator-order/browser.snapshot"
        ))
        .unwrap();
        let before = engine.navigator_order();
        assert_eq!(before.get(&a.to_string()).map(String::as_str), Some("v"));
        prepare_order_update(&mut engine, &[a, b, c], &[a, b, c], a, Some(b)).unwrap();
        let after = engine.navigator_order();
        assert_eq!(after.get(&a.to_string()).map(String::as_str), Some("m"));
        assert_eq!(after.get(&b.to_string()), before.get(&b.to_string()));
        assert_eq!(after.get(&c.to_string()), before.get(&c.to_string()));
        let snapshot = engine.semantic_snapshot().unwrap();
        assert_eq!(snapshot.nodes.get(a.to_string().as_str()).unwrap().order_key, "m");
        let frozen = engine.legacy_package_semantic_snapshot_v1().unwrap();
        assert_ne!(frozen.nodes.get(a.to_string().as_str()).unwrap().order_key, "m");
        if let Some(path) = std::env::var_os("OPENPR_FLOW_NAVIGATOR_BRIDGE_OUTPUT") {
            std::fs::write(path, engine.export_snapshot().unwrap()).unwrap();
        }
    }
    #[test]
    fn subsequent_browser_map_only_reorder_updates_current_semantics_without_rewriting_v1() {
        let engine = LoroCollabEngine::load(include_bytes!(
            "../../../../frontend/tests/fixtures/flow-navigator-order/browser-reedit.snapshot"
        ))
        .unwrap();
        let c = "33333333-3333-4333-8333-333333333333";
        assert_eq!(engine.navigator_order().get(c).map(String::as_str), Some("9"));
        assert_eq!(engine.semantic_snapshot().unwrap().nodes.get(c).unwrap().order_key, "9");
        assert_ne!(
            engine
                .legacy_package_semantic_snapshot_v1()
                .unwrap()
                .nodes
                .get(c)
                .unwrap()
                .order_key,
            "9"
        );
        assert_eq!(
            engine
                .legacy_package_semantic_snapshot_v2()
                .unwrap()
                .nodes
                .get(c)
                .unwrap()
                .order_key,
            engine
                .legacy_package_semantic_snapshot_v1()
                .unwrap()
                .nodes
                .get(c)
                .unwrap()
                .order_key
        );
    }
    #[tokio::test]
    async fn move_rejection_does_not_disclose_unreadable_navigator_metadata() {
        use axum::response::IntoResponse;
        let error = move_rejection(write::Rejected {
            update_id: None,
            code: super::super::collab::frame::RejectedCode::StaleFrontier,
            recoverable: true,
            write_state: super::super::collab::frame::WriteState::NotApplied,
            details: Some(json!({"document_id":"PRIVATE_NAVIGATOR_SENTINEL","minimum_snapshot_seq":77})),
            current_seq: Some(77),
            current_frontier: Some(b"PRIVATE_FRONTIER_SENTINEL".to_vec()),
        });
        assert_eq!(error.kind(), ApiErrorKind::StaleFrontier);
        let bytes = axum::body::to_bytes(error.into_response().into_body(), 4096)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("PRIVATE_"));
        assert!(!text.contains("current_seq"));
        assert!(!text.contains("current_frontier"));
        assert!(!text.contains("minimum_snapshot_seq"));
    }
}
