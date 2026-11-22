//! A closed, explicitly selected graph of Pages and Forms definitions.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

use collab_core::{CollabEngine, Frontier, LoroCollabEngine, Operation, SemanticSnapshot};
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    body_assets::{self, AssetKind, ValidatedUpload},
    collab::{
        authz::{self, PermissionLevel},
        bootstrap,
    },
    package::{
        self, ExportPackageManifest, ExportPolicy, PackageCounts, PackageEngine, PackageHistory, PackageMemberInput,
        PackageProducer, PackageSource,
    },
    page_transfer_ir::{Block, Content},
    portable_page_package::{
        asset_kind, asset_path, canonicalize, page, read_blocks, reconstruct, same_portable_blocks, validate_graph,
    },
    repository::{self, NewCollabDocument, NewProjection},
    scope::{self, ObjectIdentity},
};
use crate::{
    error::ApiError,
    forms::{
        native_create::{self, NativeFormCreateRequest},
        schema,
    },
};

const MAX_PAGES: usize = 16;
const MAX_FORMS: usize = 16;
const MAX_VERSIONS: u16 = 256;
const MAX_RELATIONS: u16 = 256;
/// The mark edits (a span cleared or set on a text) one package may make the replay of its
/// versions apply, over all of its Pages. Every version of a Page that keeps its layout clears the
/// spans of the version before and sets the spans of the next one on the same texts, and the style
/// anchors of every edit stay in the text. The replay is the server's own CPU, so its work is
/// bounded by a count that the package alone decides, refused before any replay (and at export, so
/// that a package this product writes is one it reads again). 16,384 edits take about 2.5 s in an
/// unoptimized build and a fraction of that in the release build; a history in which every version
/// reformats 32 spans over 256 versions fits.
const MAX_REPLAY_MARK_EDITS: u64 = 16_384;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    page_ids: Vec<Uuid>,
    form_ids: Vec<Uuid>,
    include_history: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Graph {
    pages: Vec<Page>,
    forms: Vec<Form>,
    edges: Vec<Edge>,
    references: Vec<FormReference>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Page {
    id: Uuid,
    parent: Option<Uuid>,
    versions: Vec<Version>,
    assets: Vec<Asset>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Version {
    source_seq: i64,
    source_recorded_at: Option<Timestamp>,
    block_keys: Vec<String>,
    title: String,
    blocks: Vec<Block>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    id: Uuid,
    kind: AssetKind,
    filename: String,
    content_type: String,
}

#[derive(Deserialize, Serialize, sea_orm::FromQueryResult)]
#[serde(deny_unknown_fields)]
struct Form {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    key: String,
    name: String,
    description: String,
    icon: Option<String>,
    color: Option<String>,
    title_template: String,
    schema: Value,
    detail_layout: Value,
}

#[derive(Deserialize, Serialize, sea_orm::FromQueryResult)]
#[serde(deny_unknown_fields)]
struct Edge {
    source_object_id: Uuid,
    target_object_id: Uuid,
    relation_type: String,
    position_key: String,
    properties: Value,
}

#[derive(Deserialize, Serialize, sea_orm::FromQueryResult)]
#[serde(deny_unknown_fields)]
struct FormReference {
    source_object_id: Uuid,
    target_type: String,
    target_id: Uuid,
    display: Value,
}

struct Candidate {
    graph: std::sync::Arc<Graph>,
    hash: String,
    source_scope: Uuid,
    includes_history: bool,
    uploads: BTreeMap<Uuid, ValidatedUpload>,
}

fn invalid() -> ApiError {
    ApiError::unsupported_format("Invalid portable Page and Form graph")
}

fn outside_graph() -> ApiError {
    ApiError::policy_rejected_with_details(
        "A Page or Form references a target outside the explicitly selected package",
        json!({"reason":"portable_graph_target_not_selected"}),
    )
}

// Result::map_err passes the engine error by value.
#[allow(clippy::needless_pass_by_value)]
fn map(error: collab_core::CollabError) -> ApiError {
    super::command::map_collab_error(&error)
}

fn limits() -> ApiError {
    ApiError::limit_exceeded(
        "Portable graph package exceeds a supported limit",
        "portable_graph_package",
        None,
        None,
        None,
    )
}

fn history_limit() -> ApiError {
    ApiError::limit_exceeded(
        "A Page exceeds the number of retained versions supported by one archive",
        "portable_graph_page_versions",
        Some(json!(MAX_VERSIONS)),
        None,
        None,
    )
}
fn replay_work_limit(observed: u64) -> ApiError {
    ApiError::limit_exceeded(
        "The versions of the package need more mark edits to replay than one package may carry",
        "portable_graph_replay_work",
        Some(json!(MAX_REPLAY_MARK_EDITS)),
        Some(json!(observed)),
        None,
    )
}

/// The mark and text edits that [`frames`] applies to replay the versions of `page`: the spans of
/// its first version, and for every later version either the edits of each block that changed
/// (the spans cleared, the text replaced, the spans set) when it keeps the layout of the version
/// before, or the spans of the blocks it inserts. Decided by the package's own data, the same way
/// [`frames`] decides, so it is known before any engine runs.
fn replay_mark_edits(page: &Page) -> u64 {
    let spans = |blocks: &[Block]| blocks.iter().map(|block| block.spans.len() as u64).sum::<u64>();
    let Some(first) = page.versions.first() else {
        return 0;
    };
    let mut edits = spans(&first.blocks);
    for (prior, version) in page.versions.iter().zip(page.versions.iter().skip(1)) {
        if same_layout(prior, version, &prior.blocks, &version.blocks) {
            for (before, after) in prior.blocks.iter().zip(&version.blocks) {
                if before.text == after.text && before.spans == after.spans {
                    continue;
                }
                edits += before.spans.len() as u64 + after.spans.len() as u64;
                if before.text != after.text {
                    edits += u64::from(!before.text.is_empty()) + u64::from(!after.text.is_empty());
                }
            }
        } else {
            edits += spans(&version.blocks);
        }
    }
    edits
}

async fn enabled<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    if !scope::capabilities(db).await?.portable_graph_packages_supported {
        return Err(ApiError::typed_with_details(
            crate::error::ApiErrorKind::FeatureDisabled,
            "Portable graph packages are disabled",
            json!({"reason":"portable_graph_packages_disabled"}),
        ));
    }
    scope::require_reader_ready(db).await
}

async fn ensure_admin<C: ConnectionTrait>(db: &C, workspace: Uuid, actor: Uuid) -> Result<(), ApiError> {
    if db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT m.user_id FROM workspace_members m JOIN users u ON u.id=m.user_id
         WHERE m.workspace_id=$1 AND m.user_id=$2 AND m.role IN ('owner','admin') AND u.is_active AND u.entity_type='human'",
        vec![workspace.into(), actor.into()])).await?.is_none()
    {
        return Err(ApiError::not_found("Forms definition or destination unavailable"));
    }
    Ok(())
}

async fn form_workspaces<C: ConnectionTrait>(db: &C, ids: &[Uuid], actor: Uuid) -> Result<Vec<(Uuid, i64)>, ApiError> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id FROM project_forms WHERE id=ANY($1) AND archived_at IS NULL ORDER BY workspace_id,id",
            vec![ids.to_vec().into()],
        ))
        .await?;
    if rows.len() != ids.len() {
        return Err(ApiError::not_found("Forms definition unavailable"));
    }
    let workspaces = rows
        .into_iter()
        .map(|row| row.try_get::<Uuid>("", "workspace_id"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut authorities = Vec::new();
    for workspace in workspaces {
        ensure_admin(db, workspace, actor).await?;
        authorities.push((workspace, authz::read_epoch(db, workspace).await?));
    }
    Ok(authorities)
}

fn bound_count<T: Ord + Copy>(values: &[T], maximum: usize) -> Result<BTreeSet<T>, ApiError> {
    let unique = values.iter().copied().collect::<BTreeSet<_>>();
    if values.len() != unique.len() || values.len() > maximum {
        return Err(limits());
    }
    Ok(unique)
}

fn refs(versions: &[Version]) -> impl Iterator<Item = (Uuid, AssetKind)> + '_ {
    versions
        .iter()
        .flat_map(|version| &version.blocks)
        .filter_map(|block| Some((block.asset()?, asset_kind(block)?)))
}

pub async fn export(
    db: &DatabaseConnection,
    actor: Uuid,
    anchor: Uuid,
    input: ExportInput,
) -> Result<Vec<u8>, ApiError> {
    use sea_orm::FromQueryResult;
    enabled(db).await?;
    let selected = bound_count(&input.page_ids, MAX_PAGES)?;
    let forms_selected = bound_count(&input.form_ids, MAX_FORMS)?;
    if !selected.contains(&anchor) {
        return Err(ApiError::invalid_update(
            "Package selection must include the source Page",
        ));
    }
    let (source, _) = page(db, anchor, actor, PermissionLevel::View).await?;
    let mut proofs = Vec::with_capacity(selected.len());
    for id in &selected {
        let (identity, proof) = page(db, *id, actor, PermissionLevel::View).await?;
        if identity.scope_id != source.scope_id {
            return Err(outside_graph());
        }
        proofs.push(proof);
    }
    let workspaces = form_workspaces(db, &input.form_ids, actor).await?;
    let tx = db.begin().await?;
    enabled(&tx).await?;
    scope::fence_many_with_workspaces(&tx, &proofs, &workspaces).await?;
    for (workspace, _) in &workspaces {
        ensure_admin(&tx, *workspace, actor).await?;
    }
    tx.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects WHERE id=ANY($1) ORDER BY id FOR SHARE",
        vec![input.page_ids.clone().into()],
    ))
    .await?;
    tx.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=ANY($1) ORDER BY id FOR SHARE",
        vec![input.page_ids.clone().into()],
    ))
    .await?;

    let mut pages = Vec::new();
    let mut members = Vec::new();
    let mut asset_bytes = 0usize;
    for id in &selected {
        let (identity, _) = page(&tx, *id, actor, PermissionLevel::View).await?;
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM collab_documents WHERE object_id=$1",
                vec![(*id).into()],
            ))
            .await?
            .ok_or_else(invalid)?;
        let document: Uuid = row.try_get("", "id")?;
        let versions = export_versions(db, &tx, document, input.include_history).await?;
        for block in versions.iter().flat_map(|version| &version.blocks) {
            if let Content::ChildPage { child_page_id } = block.content
                && (child_page_id == *id || !selected.contains(&child_page_id))
            {
                return Err(outside_graph());
            }
        }
        let mut assets = Vec::new();
        let ids = refs(&versions).collect::<BTreeMap<_, _>>();
        for (asset_id, kind) in ids {
            let row = tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT filename,content_type,byte_size,content_sha256,bytes FROM flow_page_assets
                 WHERE id=$1 AND object_id=$2 AND scope_id=$3 AND kind=$4 AND state IN ('active','retained') AND first_referenced_seq IS NOT NULL FOR SHARE",
                vec![asset_id.into(),(*id).into(),source.scope_id.into(),kind.as_str().into()])).await?.ok_or_else(|| {
                    ApiError::policy_rejected("A selected version has an unavailable media binding")
                })?;
            let filename: String = row.try_get("", "filename")?;
            let content_type: String = row.try_get("", "content_type")?;
            let upload = body_assets::validate_bytes(
                kind,
                &filename,
                Some(&content_type),
                row.try_get("", "bytes")?,
                body_assets::AssetLimits::configured().max_bytes,
            )?;
            asset_bytes = asset_bytes.saturating_add(upload.bytes.len());
            if members.len() >= package::PORTABLE_PAGE_ASSET_COUNT || asset_bytes > package::PORTABLE_PAGE_BYTES {
                return Err(limits());
            }
            if upload.content_type != content_type
                || upload.sha256 != row.try_get::<String>("", "content_sha256")?
                || Some(upload.bytes.len()) != usize::try_from(row.try_get::<i32>("", "byte_size")?).ok()
            {
                return Err(ApiError::checksum_mismatch(
                    "Stored Page asset disagrees with its metadata",
                ));
            }
            assets.push(Asset {
                id: asset_id,
                kind,
                filename,
                content_type,
            });
            members.push(PackageMemberInput {
                path: asset_path(asset_id),
                kind: "page_asset".into(),
                bytes: upload.bytes,
            });
        }
        pages.push(Page {
            id: *id,
            parent: identity.parent_id.filter(|parent| selected.contains(parent)),
            versions,
            assets,
        });
    }
    let forms = Form::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,workspace_id,project_id,key,name,description,icon,color,title_template,schema,detail_layout
         FROM project_forms WHERE id=ANY($1) AND archived_at IS NULL ORDER BY id FOR SHARE",
        vec![input.form_ids.clone().into()],
    ))
    .all(&tx)
    .await?;
    if forms.len() != forms_selected.len() {
        return Err(ApiError::not_found("Forms definition unavailable"));
    }
    let edges = Edge::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_object_id,target_object_id,relation_type,position_key,properties FROM flow_relations
         WHERE source_object_id=ANY($1) ORDER BY id LIMIT $2",
        vec![input.page_ids.clone().into(), (i64::from(MAX_RELATIONS) + 1).into()],
    ))
    .all(&tx)
    .await?;
    let references = FormReference::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_object_id,target_type,target_id,display FROM flow_bridge_references
         WHERE source_object_id=ANY($1) AND removed_at IS NULL ORDER BY id LIMIT $2",
        vec![input.page_ids.into(), (i64::from(MAX_RELATIONS) + 1).into()],
    ))
    .all(&tx)
    .await?;
    let graph = Graph {
        pages,
        forms,
        edges,
        references,
    };
    validate(&graph, input.include_history)?;
    let now = Timestamp::now().to_rfc3339();
    let roots = graph
        .pages
        .iter()
        .filter(|page| page.parent.is_none())
        .map(|page| page.id.to_string())
        .collect();
    let content = serde_jcs::to_vec(&graph).map_err(|_| ApiError::internal())?;
    if content.len() as u64 > package::PORTABLE_GRAPH_CONTENT_BYTES
        || asset_bytes.saturating_add(content.len()) > package::PORTABLE_PAGE_BYTES
    {
        return Err(limits());
    }
    members.push(PackageMemberInput {
        path: package::PORTABLE_GRAPH_PATH.into(),
        kind: "graph_content".into(),
        bytes: content,
    });
    let manifest = ExportPackageManifest {
        schema: package::PORTABLE_GRAPH_SCHEMA.into(),
        package_id: Uuid::new_v4().to_string(),
        created_at: now.clone(),
        producer: PackageProducer {
            product: "sylvode".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            source_head: env!("OPENPR_EMBEDDED_GIT_COMMIT").into(),
        },
        source: PackageSource {
            workspace_id: source.workspace_id.map(|id| id.to_string()),
            scope_id: Some(source.scope_id.to_string()),
            scope: "scope".into(),
            root_object_ids: roots,
        },
        flow_schema_version: 2,
        engine: PackageEngine {
            name: package::ENGINE_NAME.into(),
            crate_version: package::ENGINE_CRATE_VERSION.into(),
            wire_format_version: package::ENGINE_WIRE_FORMAT_VERSION,
            semantic_model: Some(package::SEMANTIC_MODEL_V5.into()),
        },
        history: PackageHistory {
            included: input.include_history,
            snapshot_history: Some(
                if input.include_history {
                    "typed_versions"
                } else {
                    "excluded"
                }
                .into(),
            ),
            through_seq_by_document: BTreeMap::new(),
        },
        counts: counts(&graph),
        members: Vec::new(),
        export_policy: ExportPolicy {
            complete: true,
            permission_snapshot_at: now,
        },
        projects: Vec::new(),
        warnings: vec!["explicit_closed_graph".into(), "forms_definitions_only".into()],
    };
    let built = package::build_package(manifest, members)?;
    if built.bytes.len() > package::PORTABLE_PAGE_BYTES {
        return Err(limits());
    }
    tx.commit().await?;
    Ok(built.bytes)
}

async fn export_versions(
    db: &DatabaseConnection,
    tx: &DatabaseTransaction,
    document: Uuid,
    history: bool,
) -> Result<Vec<Version>, ApiError> {
    let boot = bootstrap::load(db, document).await?;
    if !history {
        let mut head = super::document::engine_from_boot(&boot)?;
        return Ok(vec![capture_version(&mut head, boot.head_seq, None)?]);
    }
    let rows = tx
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT seq,after_frontier,created_at FROM collab_updates WHERE document_id=$1 ORDER BY seq LIMIT $2",
            vec![document.into(), (i64::from(MAX_VERSIONS) + 1).into()],
        ))
        .await?;
    let timestamps = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<i64>("", "seq")?,
                row.try_get::<Timestamp>("", "created_at")?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, sea_orm::DbErr>>()?;
    if rows.len() > usize::from(MAX_VERSIONS) {
        return Err(history_limit());
    }
    let mut versions = Vec::new();
    if rows
        .iter()
        .any(|row| row.try_get::<i64>("", "seq").is_ok_and(|seq| seq < boot.snapshot_seq))
    {
        let head = super::document::engine_from_boot(&boot)?;
        for row in &rows {
            let seq = row.try_get::<i64>("", "seq")?;
            if seq >= boot.snapshot_seq {
                break;
            }
            let mut fork = head
                .fork_at_frontier(&Frontier::from_bytes(row.try_get("", "after_frontier")?))
                .map_err(map)?;
            versions.push(capture_version(&mut fork, seq, timestamps.get(&seq).copied())?);
        }
    }
    let mut replay = LoroCollabEngine::load(&boot.snapshot).map_err(map)?;
    let mut capture = |engine: &mut LoroCollabEngine, seq| {
        versions.push(capture_version(engine, seq, timestamps.get(&seq).copied())?);
        Ok::<(), ApiError>(())
    };
    // Accepted updates can precede initial editor admission. Capture those through a read-only
    // fork so admitting a legacy rendering never changes the operation history of the replay.
    capture_retained(&mut replay, boot.snapshot_seq, &mut capture)?;
    for update in &boot.tail_updates {
        replay.import_update(&update.bytes).map_err(map)?;
        capture_retained(&mut replay, update.seq, &mut capture)?;
    }
    if versions.len() > usize::from(MAX_VERSIONS) {
        return Err(history_limit());
    }
    Ok(versions)
}

fn capture_retained(
    engine: &mut LoroCollabEngine,
    seq: i64,
    capture: &mut impl FnMut(&mut LoroCollabEngine, i64) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if engine.has_editor_doc().map_err(map)? {
        capture(engine, seq)
    } else {
        let mut clone = LoroCollabEngine::load(&engine.export_snapshot().map_err(map)?).map_err(map)?;
        capture(&mut clone, seq)
    }
}

fn capture_version(
    selected: &mut LoroCollabEngine,
    source_seq: i64,
    source_recorded_at: Option<Timestamp>,
) -> Result<Version, ApiError> {
    let build = || {
        if !selected.has_editor_doc().map_err(map)? {
            selected.initialize_page_editor().map_err(map)?;
        }
        let mut blocks = read_blocks(&selected.semantic_snapshot().map_err(map)?)?;
        let block_keys = blocks
            .iter()
            .map(|block| bootstrap::content_hash(block.source_cid.as_bytes()))
            .collect();
        canonicalize(&mut blocks)?;
        let title = selected.title().map_err(map)?;
        validate_graph(&title, &blocks)?;
        Ok::<_, ApiError>(Version {
            source_seq,
            source_recorded_at,
            block_keys,
            title,
            blocks,
        })
    };
    let mut build = build;
    build().map_err(|error| error.with_detail("source_seq", json!(source_seq)))
}

fn counts(graph: &Graph) -> PackageCounts {
    PackageCounts {
        objects: (graph.pages.len() + graph.forms.len()) as u64,
        documents: graph.pages.len() as u64,
        updates: graph
            .pages
            .iter()
            .map(|page| page.versions.len().saturating_sub(1) as u64)
            .sum(),
        relations: (graph.edges.len() + graph.references.len()) as u64,
        ..PackageCounts::default()
    }
}

fn parent_order(graph: &Graph) -> Result<Vec<&Page>, ApiError> {
    let mut remaining = graph
        .pages
        .iter()
        .map(|page| (page.id, page))
        .collect::<BTreeMap<_, _>>();
    let mut parents = BTreeSet::new();
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let page = remaining
            .values()
            .find(|page| page.parent.is_none_or(|id| parents.contains(&id)))
            .copied()
            .ok_or_else(invalid)?;
        remaining.remove(&page.id);
        parents.insert(page.id);
        ordered.push(page);
    }
    Ok(ordered)
}

/// The Form relation configuration is project-local and explicitly keyed by its target Form.
/// Other field types that can embed cross-resource behavior need a separate typed mapping.
fn remap_form_schema(
    form: &Form,
    targets: &BTreeMap<(Uuid, &str), Uuid>,
    new_keys: Option<&BTreeMap<Uuid, String>>,
) -> Result<Value, ApiError> {
    schema::validate_schema(&form.schema).map_err(ApiError::invalid_update)?;
    let mut schema = form.schema.clone();
    let fields = schema
        .get_mut("fields")
        .and_then(Value::as_array_mut)
        .ok_or_else(invalid)?;
    for field in fields {
        match field.get("type").and_then(Value::as_str) {
            Some("relation") => {
                let key = field
                    .pointer("/relation/form_key")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                let target = targets.get(&(form.project_id, key)).ok_or_else(outside_graph)?;
                if let Some(keys) = new_keys {
                    *field.pointer_mut("/relation/form_key").ok_or_else(invalid)? =
                        json!(keys.get(target).ok_or_else(invalid)?);
                }
            }
            Some("child_table" | "formula" | "ai_summary") => {
                return Err(ApiError::unsupported_format(
                    "Selected Form field needs an unavailable portable mapping",
                ));
            }
            _ => {}
        }
    }
    Ok(schema)
}

fn validate(graph: &Graph, includes_history: bool) -> Result<(), ApiError> {
    let pages = bound_count(&graph.pages.iter().map(|page| page.id).collect::<Vec<_>>(), MAX_PAGES)?;
    let forms = bound_count(&graph.forms.iter().map(|form| form.id).collect::<Vec<_>>(), MAX_FORMS)?;
    if pages.is_empty()
        || graph.edges.len() + graph.references.len() > usize::from(MAX_RELATIONS)
        || pages.iter().chain(&forms).any(Uuid::is_nil)
    {
        return Err(invalid());
    }
    parent_order(graph)?;
    let mut global_assets = BTreeSet::new();
    for page in &graph.pages {
        if page.versions.is_empty()
            || page.versions.len() > usize::from(MAX_VERSIONS)
            || (!includes_history && page.versions.len() != 1)
            || page.versions.first().is_some_and(|version| version.source_seq < 0)
            || page
                .versions
                .array_windows()
                .any(|[earlier, later]| earlier.source_seq >= later.source_seq)
        {
            return Err(invalid());
        }
        let mut page_assets = BTreeMap::new();
        for asset in &page.assets {
            if asset.id.is_nil()
                || !global_assets.insert(asset.id)
                || global_assets.len() > package::PORTABLE_PAGE_ASSET_COUNT
            {
                return Err(invalid());
            }
            page_assets.insert(asset.id, asset.kind);
        }
        let mut referenced = BTreeSet::new();
        for (id, kind) in refs(&page.versions) {
            if page_assets.get(&id) != Some(&kind) {
                return Err(invalid());
            }
            referenced.insert(id);
        }
        if referenced.len() != page.assets.len() {
            return Err(invalid());
        }
        for version in &page.versions {
            validate_graph(&version.title, &version.blocks)?;
            let ids = version.block_keys.iter().collect::<BTreeSet<_>>();
            if ids.len() != version.blocks.len()
                || ids.len() != version.block_keys.len()
                || ids
                    .iter()
                    .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')))
            {
                return Err(invalid());
            }
            for block in &version.blocks {
                if let Content::ChildPage { child_page_id } = block.content
                    && (child_page_id == page.id || !pages.contains(&child_page_id))
                {
                    return Err(outside_graph());
                }
            }
        }
    }
    let mut edges = BTreeSet::new();
    for edge in &graph.edges {
        if !pages.contains(&edge.source_object_id) || !pages.contains(&edge.target_object_id) {
            return Err(outside_graph());
        }
        if edge.source_object_id == edge.target_object_id
            || !super::relations::valid_relation_type(&edge.relation_type)
            || edge.relation_type == "embeds"
            || !edge.properties.is_object()
            || !edges.insert((edge.source_object_id, edge.target_object_id, &edge.relation_type))
        {
            return Err(invalid());
        }
    }
    for reference in &graph.references {
        if reference.target_type != "form"
            || !pages.contains(&reference.source_object_id)
            || !forms.contains(&reference.target_id)
        {
            return Err(outside_graph());
        }
        if !reference.display.is_object() {
            return Err(invalid());
        }
    }
    let replay_edits = graph.pages.iter().map(replay_mark_edits).sum::<u64>();
    if replay_edits > MAX_REPLAY_MARK_EDITS {
        return Err(replay_work_limit(replay_edits));
    }
    let targets = graph
        .forms
        .iter()
        .map(|form| ((form.project_id, form.key.as_str()), form.id))
        .collect::<BTreeMap<_, _>>();
    if targets.len() != graph.forms.len() {
        return Err(invalid());
    }
    for form in &graph.forms {
        if form.name.trim().is_empty() || !form.detail_layout.is_object() {
            return Err(invalid());
        }
        remap_form_schema(form, &targets, None)?;
    }
    Ok(())
}

fn decode(bytes: &[u8], expected: Option<&str>) -> Result<Candidate, ApiError> {
    let verified = package::verify_portable_graph(bytes, expected)?;
    let manifest = verified.manifest;
    let includes_history = manifest.history.included;
    let source_scope = manifest
        .source
        .scope_id
        .as_deref()
        .and_then(|id| id.parse().ok())
        .ok_or_else(invalid)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let graph: Graph = serde_json::from_reader(archive.by_name(package::PORTABLE_GRAPH_PATH).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    validate(&graph, includes_history)?;
    let roots = graph
        .pages
        .iter()
        .filter(|page| page.parent.is_none())
        .map(|page| page.id.to_string())
        .collect::<BTreeSet<_>>();
    if manifest.counts != counts(&graph)
        || manifest.source.root_object_ids.iter().collect::<BTreeSet<_>>() != roots.iter().collect()
        || manifest.members.len() != 1 + graph.pages.iter().map(|page| page.assets.len()).sum::<usize>()
    {
        return Err(invalid());
    }
    let mut uploads = BTreeMap::new();
    let max_bytes = body_assets::AssetLimits::configured().max_bytes;
    for asset in graph.pages.iter().flat_map(|page| &page.assets) {
        let mut member = archive.by_name(&asset_path(asset.id)).map_err(|_| invalid())?;
        if member.size() > max_bytes {
            return Err(limits());
        }
        let mut data = Vec::new();
        member.read_to_end(&mut data).map_err(|_| invalid())?;
        let upload =
            body_assets::validate_bytes(asset.kind, &asset.filename, Some(&asset.content_type), data, max_bytes)?;
        if upload.filename != asset.filename || upload.content_type != asset.content_type {
            return Err(invalid());
        }
        uploads.insert(asset.id, upload);
    }
    Ok(Candidate {
        graph: std::sync::Arc::new(graph),
        hash: verified.package_sha256,
        source_scope,
        includes_history,
        uploads,
    })
}

async fn destination_forms(
    tx: &DatabaseTransaction,
    destination: &ObjectIdentity,
    actor: Uuid,
    has_forms: bool,
) -> Result<Option<(Uuid, Uuid)>, ApiError> {
    if !has_forms {
        return Ok(None);
    }
    let (Some(workspace), Some(project)) = (destination.workspace_id, destination.project_id) else {
        return Err(ApiError::invalid_update(
            "A graph containing Forms requires a destination Page in a managed project",
        ));
    };
    ensure_admin(tx, workspace, actor).await?;
    if !super::bridge::bridge_enabled(tx, workspace).await? {
        return Err(ApiError::policy_rejected("destination Forms bridge is disabled"));
    }
    let project_exists = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE id=$1 AND workspace_id=$2 FOR SHARE",
            vec![project.into(), workspace.into()],
        ))
        .await?;
    if project_exists.is_none() {
        return Err(ApiError::not_found("Forms destination project unavailable"));
    }
    Ok(Some((workspace, project)))
}

pub async fn preview(
    db: &DatabaseConnection,
    actor: Uuid,
    anchor: Uuid,
    bytes: &[u8],
    export_review: bool,
) -> Result<Value, ApiError> {
    enabled(db).await?;
    let level = if export_review {
        PermissionLevel::View
    } else {
        PermissionLevel::Edit
    };
    let (destination, proof) = page(db, anchor, actor, level).await?;
    let candidate = decode(bytes, None)?;
    let graph = &candidate.graph;
    let mut proofs = vec![proof];
    let workspace_proofs = if export_review {
        if destination.scope_id != candidate.source_scope || !graph.pages.iter().any(|page| page.id == anchor) {
            return Err(outside_graph());
        }
        for item in &graph.pages {
            let (identity, proof) = page(db, item.id, actor, PermissionLevel::View).await?;
            if identity.scope_id != candidate.source_scope {
                return Err(outside_graph());
            }
            proofs.push(proof);
        }
        form_workspaces(db, &graph.forms.iter().map(|form| form.id).collect::<Vec<_>>(), actor).await?
    } else {
        Vec::new()
    };
    // The replay is validated (and bounded) here, off the executor; the frames are built again at
    // import.
    build_timelines(
        std::sync::Arc::clone(&candidate.graph),
        std::sync::Arc::new(ImportPlan::new(graph)),
    )
    .await?;
    let tx = db.begin().await?;
    enabled(&tx).await?;
    scope::fence_many_with_workspaces(&tx, &proofs, &workspace_proofs).await?;
    if export_review {
        for (workspace, _) in workspace_proofs {
            ensure_admin(&tx, workspace, actor).await?;
        }
    } else {
        let (current, _) = page(&tx, anchor, actor, PermissionLevel::Edit).await?;
        super::page_transfers::require_transfer_write(&tx, &current).await?;
        authz::ensure_scope_parent_can_adopt_child(&tx, current.scope_id, anchor).await?;
        destination_forms(&tx, &current, actor, !graph.forms.is_empty()).await?;
    }
    tx.commit().await?;
    let pages = graph
        .pages
        .iter()
        .map(|page| {
            let head = page.versions.last().ok_or_else(invalid)?;
            let excerpts = head
                .blocks
                .iter()
                .filter_map(|block| {
                    let text = match &block.content {
                        Content::Image { alt, .. } => alt,
                        Content::Attachment { name, .. } => name,
                        _ => &block.text,
                    };
                    (!text.is_empty()).then(|| text.chars().take(160).collect::<String>())
                })
                .take(8)
                .collect::<Vec<_>>();
            Ok(
                json!({"source_object_id":page.id,"source_parent_id":page.parent,"title":head.title,
                "asset_count":page.assets.len(),"version_count":page.versions.len(),"excerpts":excerpts,
                "source_sequences":page.versions.iter().map(|version|version.source_seq).collect::<Vec<_>>() }),
            )
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let forms = graph
        .forms
        .iter()
        .map(|form| json!({"source_form_id":form.id,"name":form.name}))
        .collect::<Vec<_>>();
    let mut references = graph
        .references
        .iter()
        .map(|reference| {
            json!({"kind":"form",
        "source_object_id":reference.source_object_id,"target_form_id":reference.target_id,"mapping":"new"})
        })
        .collect::<Vec<_>>();
    references.extend(graph.edges.iter().map(|edge| {
        json!({"kind":"relation","relation_type":edge.relation_type,
        "source_object_id":edge.source_object_id,"target_object_id":edge.target_object_id,"mapping":"new"})
    }));
    for item in &graph.pages {
        let targets = item
            .versions
            .iter()
            .flat_map(|version| &version.blocks)
            .filter_map(|block| match block.content {
                Content::ChildPage { child_page_id } => Some(child_page_id),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        references.extend(targets.into_iter().map(|target| {
            json!({"kind":"child_page","source_object_id":item.id,
            "target_object_id":target,"mapping":"new"})
        }));
    }
    Ok(
        json!({"package_sha256":candidate.hash,"pages":pages,"forms":forms,"include_history":candidate.includes_history,"references":references}),
    )
}

struct ImportPlan {
    pages: BTreeMap<Uuid, (Uuid, Uuid)>,
    forms: BTreeMap<Uuid, Uuid>,
    form_keys: BTreeMap<Uuid, String>,
    assets: BTreeMap<Uuid, Uuid>,
}

impl ImportPlan {
    fn new(graph: &Graph) -> Self {
        let pages = graph
            .pages
            .iter()
            .map(|page| (page.id, (Uuid::new_v4(), Uuid::new_v4())))
            .collect();
        let forms = graph
            .forms
            .iter()
            .map(|form| (form.id, Uuid::new_v4()))
            .collect::<BTreeMap<_, _>>();
        let form_keys = forms
            .iter()
            .map(|(&source, target)| (source, format!("portable_{}", target.simple())))
            .collect();
        let assets = graph
            .pages
            .iter()
            .flat_map(|page| &page.assets)
            .map(|asset| (asset.id, Uuid::new_v4()))
            .collect();
        Self {
            pages,
            forms,
            form_keys,
            assets,
        }
    }

    fn object(&self, id: Uuid) -> Result<Uuid, ApiError> {
        self.pages.get(&id).map(|(object, _)| *object).ok_or_else(invalid)
    }

    fn mapped_blocks(&self, version: &Version) -> Result<Vec<Block>, ApiError> {
        let mut blocks = version.blocks.clone();
        for block in &mut blocks {
            block.remap_asset(&self.assets).map_err(|_| invalid())?;
            if let Content::ChildPage { child_page_id } = &mut block.content {
                *child_page_id = self.object(*child_page_id)?;
            }
        }
        Ok(blocks)
    }
}

struct Frame {
    data: Vec<u8>,
    frontier: Vec<u8>,
    semantic: SemanticSnapshot,
}

fn frames(page: &Page, plan: &ImportPlan) -> Result<Vec<Frame>, ApiError> {
    let (object, _) = *plan.pages.get(&page.id).ok_or_else(invalid)?;
    let first = page.versions.first().ok_or_else(invalid)?;
    let blocks = plan.mapped_blocks(first)?;
    let (mut engine, semantic) = reconstruct(&first.title, &blocks, object)?;
    let mut frames = vec![Frame {
        data: engine.export_snapshot().map_err(map)?,
        frontier: engine.frontier().as_bytes().to_vec(),
        semantic,
    }];
    let mut total_bytes = frames.first().ok_or_else(invalid)?.data.len();
    for (prior, version) in page.versions.iter().zip(page.versions.iter().skip(1)) {
        let base = engine.frontier();
        let previous = read_blocks(&engine.semantic_snapshot().map_err(map)?)?;
        let expected = plan.mapped_blocks(version)?;
        if same_layout(prior, version, &previous, &expected) {
            for (existing, next) in previous.iter().zip(&expected) {
                edit_block_text(&mut engine, existing, next)?;
            }
        } else {
            super::page_transfers::insert_selected(&mut engine, &expected, None, 0, Uuid::new_v4())?;
            for block in previous.iter().filter(|block| block.parent_cid.is_none()) {
                engine
                    .apply_content_operation(&Operation::DeleteNode {
                        id: block.source_cid.as_str().into(),
                    })
                    .map_err(map)?;
            }
        }
        engine.set_title(&version.title).map_err(map)?;
        let semantic = engine.semantic_snapshot().map_err(map)?;
        let mut current = read_blocks(&semantic)?;
        canonicalize(&mut current)?;
        if !same_portable_blocks(&current, &expected) || engine.title().map_err(map)? != version.title {
            return Err(invalid());
        }
        let data = engine.export_from(&base).map_err(map)?;
        collab_core::InputLimits::default()
            .validate_update(&data)
            .map_err(map)?;
        total_bytes = total_bytes.saturating_add(data.len());
        frames.push(Frame {
            data,
            frontier: engine.frontier().as_bytes().to_vec(),
            semantic,
        });
    }
    bootstrap::check_bootstrap_decoded_bytes(total_bytes as u64)?;
    Ok(frames)
}

/// Replays the versions of every Page of `graph` into frames, off the async executor: the replay is
/// CPU work, bounded by [`MAX_REPLAY_MARK_EDITS`] (checked by [`validate`] before this runs).
async fn build_timelines(
    graph: std::sync::Arc<Graph>,
    plan: std::sync::Arc<ImportPlan>,
) -> Result<BTreeMap<Uuid, Vec<Frame>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        graph
            .pages
            .iter()
            .map(|page| Ok((page.id, frames(page, &plan)?)))
            .collect()
    })
    .await
    .map_err(|_| ApiError::internal())?
}

fn same_layout(prior: &Version, next: &Version, current: &[Block], expected: &[Block]) -> bool {
    prior.block_keys == next.block_keys
        && current.len() == expected.len()
        && prior
            .blocks
            .iter()
            .zip(&next.blocks)
            .all(|(before, after)| before.parent_cid == after.parent_cid)
        && current
            .iter()
            .zip(expected)
            .all(|(before, after)| before.content == after.content)
}

fn edit_block_text(engine: &mut LoroCollabEngine, before: &Block, after: &Block) -> Result<(), ApiError> {
    use collab_core::page_inline::MarkEdit;
    if before.text == after.text && before.spans == after.spans {
        return Ok(());
    }
    let id = before.source_cid.as_str();
    let edits = |block: &Block, clear| {
        block
            .spans
            .iter()
            .map(|span| MarkEdit {
                start: span.start,
                end: span.end,
                mark: span.mark,
                clear,
                href: if clear { None } else { span.href.clone() },
            })
            .collect::<Vec<_>>()
    };
    if !before.spans.is_empty() {
        engine.apply_page_marks(id, &edits(before, true)).map_err(map)?;
    }
    if before.text != after.text {
        if !before.text.is_empty() {
            engine
                .apply_content_operation(&Operation::DeleteText {
                    id: id.into(),
                    index: 0,
                    len: u32::try_from(before.text.len()).map_err(|_| limits())?,
                })
                .map_err(map)?;
        }
        if !after.text.is_empty() {
            engine
                .apply_content_operation(&Operation::InsertText {
                    id: id.into(),
                    index: 0,
                    text: after.text.clone(),
                })
                .map_err(map)?;
        }
    }
    if !after.spans.is_empty() {
        engine.apply_page_marks(id, &edits(after, false)).map_err(map)?;
    }
    Ok(())
}

struct PagePromotion<'a> {
    source: &'a Page,
    actor: Uuid,
    object: Uuid,
    document: Uuid,
    frames: Vec<Frame>,
    source_scope: Uuid,
    archive_hash: &'a str,
}

async fn store_timeline(tx: &DatabaseTransaction, input: PagePromotion<'_>) -> Result<(), ApiError> {
    let PagePromotion {
        source,
        actor,
        object,
        document,
        frames,
        source_scope,
        archive_hash,
    } = input;
    let identity = scope::identity(tx, object).await?;
    let mut previous = Vec::new();
    let mut body_bytes = 0i64;
    for (offset, (frame, version)) in frames.into_iter().zip(&source.versions).enumerate() {
        let seq = i64::try_from(offset).map_err(|_| limits())?;
        if seq == 0 {
            repository::insert_collab_document(
                tx,
                &NewCollabDocument {
                    id: document,
                    object_id: object,
                    format_version: "loro-1".into(),
                    snapshot: frame.data,
                    frontier: frame.frontier.clone(),
                },
            )
            .await?;
        } else {
            let key = format!("portable_graph:{object}:{seq}");
            let binding = json!({"imported":true,"source_scope_id":source_scope,"source_object_id":source.id,
                "source_seq":version.source_seq,"source_recorded_at":version.source_recorded_at,
                "package_sha256":archive_hash});
            // Archive labels are provenance, not local events authored by the original account.
            // Imported historical changes are audit rows and must not dispatch live automations.
            let event =
                super::scope_event::insert_object_audit(tx, &identity, actor, "flow.version.imported", &key, binding)
                    .await?;
            let hash = bootstrap::content_hash(&frame.data);
            body_bytes = body_bytes.saturating_add(i64::try_from(frame.data.len()).map_err(|_| limits())?);
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO collab_updates(document_id,seq,update_id,content_hash,idempotency_key,before_frontier,
                   after_frontier,bytes,actor_id,origin_surface,origin_client_id,projection_seq,event_id)
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'system','portable-graph-import',$2,$10)",
                vec![
                    document.into(),
                    seq.into(),
                    Uuid::new_v4().into(),
                    hash.into(),
                    key.into(),
                    previous.into(),
                    frame.frontier.clone().into(),
                    frame.data.into(),
                    actor.into(),
                    event.into(),
                ],
            ))
            .await?;
        }
        let refs = body_assets::typed_references(&frame.semantic)?;
        let child_refs = super::child_pages::typed_references(&frame.semantic)?;
        let registry = super::block_registry::prepare(&frame.semantic)?;
        body_assets::reconcile_accepted(tx, object, seq, &refs).await?;
        super::child_pages::reconcile_accepted(tx, object, Some(actor), seq, &child_refs).await?;
        super::block_registry::reconcile_accepted(
            tx,
            object,
            seq,
            &registry,
            super::block_registry::RegistrationAllowance::Complete,
        )
        .await?;
        if offset + 1 == source.versions.len() {
            if seq > 0 {
                tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                    "UPDATE collab_documents SET head_seq=$2,head_frontier=$3,byte_count=byte_count+$4,update_count=$2 WHERE id=$1",
                    vec![document.into(),seq.into(),frame.frontier.clone().into(),body_bytes.into()])).await?;
            }
            repository::insert_projection(
                tx,
                &NewProjection {
                    object_id: object,
                    document_seq: seq,
                    document_frontier: frame.frontier.clone(),
                    title: version.title.clone(),
                    state: super::projection::state_json(&frame.semantic).map_err(|_| ApiError::internal())?,
                    plain_text: super::projection::plain_text(&frame.semantic),
                },
            )
            .await?;
        }
        previous = frame.frontier;
    }
    Ok(())
}

pub async fn import(
    db: &DatabaseConnection,
    actor: Uuid,
    anchor: Uuid,
    bytes: &[u8],
    expected: &str,
    key: &str,
) -> Result<Value, ApiError> {
    enabled(db).await?;
    let (_, proof) = page(db, anchor, actor, PermissionLevel::Edit).await?;
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("operation key must contain 1..128 bytes"));
    }
    let mut candidate = decode(bytes, Some(expected))?;
    let graph = &candidate.graph;
    let plan = std::sync::Arc::new(ImportPlan::new(graph));
    let ordered = parent_order(graph)?;
    let mut timelines = build_timelines(std::sync::Arc::clone(&candidate.graph), std::sync::Arc::clone(&plan)).await?;
    let body = json!({"operation":"portable_graph_import","parent_object_id":anchor,"package_sha256":expected});
    let tx = db.begin().await?;
    enabled(&tx).await?;
    scope::fence_many(&tx, &[proof]).await?;
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects WHERE id=$1 FOR UPDATE",
        vec![anchor.into()],
    ))
    .await?;
    let (destination, _) = page(&tx, anchor, actor, PermissionLevel::Edit).await?;
    super::page_transfers::require_transfer_write(&tx, &destination).await?;
    authz::ensure_scope_parent_can_adopt_child(&tx, destination.scope_id, anchor).await?;
    let form_destination = destination_forms(&tx, &destination, actor, !graph.forms.is_empty()).await?;
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, destination.scope_id, actor, key, &body).await? {
        tx.commit().await?;
        return Ok(prior);
    }
    let mut forms = Vec::new();
    if let Some((workspace, project)) = form_destination {
        let targets = graph
            .forms
            .iter()
            .map(|form| ((form.project_id, form.key.as_str()), form.id))
            .collect::<BTreeMap<_, _>>();
        for form in &graph.forms {
            let form_id = *plan.forms.get(&form.id).ok_or_else(invalid)?;
            let key = plan.form_keys.get(&form.id).ok_or_else(invalid)?.clone();
            native_create::create_form_in_transaction(&tx,NativeFormCreateRequest{
                form_id:Some(form_id),workspace_id:workspace,project_id:project,key,name:form.name.clone(),
                description:form.description.clone(),icon:form.icon.clone(),color:form.color.clone(),
                title_template:Some(form.title_template.clone()),schema:remap_form_schema(form,&targets,Some(&plan.form_keys))?,
                detail_layout:form.detail_layout.clone(),created_by:Some(actor),
                source:json!({"type":"user","actor_id":actor,"origin":"portable_graph_import","source_form_id":form.id}),
                change_summary:"Imported portable Form definition".into(),
            }).await?;
            forms.push(json!({"source_form_id":form.id,"form_id":form_id}));
        }
    }
    for item in &ordered {
        let object = plan.object(item.id)?;
        let parent = item.parent.map(|id| plan.object(id)).transpose()?.unwrap_or(anchor);
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO flow_objects(id,scope_id,workspace_id,project_id,object_type,parent_id,created_by,governance_metadata)
             VALUES($1,$2,$3,$4,'page',$5,$6,'{}'::jsonb)",
            vec![object.into(),destination.scope_id.into(),destination.workspace_id.into(),destination.project_id.into(),
                parent.into(),actor.into()])).await?;
    }
    let mut pages = Vec::new();
    let mut first_created = None;
    for item in ordered {
        let (object, document) = *plan.pages.get(&item.id).ok_or_else(invalid)?;
        for asset in &item.assets {
            let upload = candidate.uploads.remove(&asset.id).ok_or_else(invalid)?;
            let id = *plan.assets.get(&asset.id).ok_or_else(invalid)?;
            let size = i32::try_from(upload.bytes.len()).map_err(|_| limits())?;
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                "INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by)
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                vec![id.into(),destination.scope_id.into(),object.into(),upload.kind.as_str().into(),upload.filename.into(),
                    upload.content_type.into(),size.into(),upload.sha256.into(),upload.bytes.into(),actor.into()])).await?;
        }
        store_timeline(
            &tx,
            PagePromotion {
                source: item,
                actor,
                object,
                document,
                source_scope: candidate.source_scope,
                archive_hash: &candidate.hash,
                frames: timelines.remove(&item.id).ok_or_else(invalid)?,
            },
        )
        .await?;
        let identity = scope::identity(&tx, object).await?;
        let event = super::scope_event::insert_for_object(
            &tx,
            &identity,
            actor,
            "flow.object.created",
            "flow_object",
            object,
            &format!("portable_graph_create:{object}"),
            json!({"object_id":object,"object_type":"page","parent_object_id":identity.parent_id}),
            json!({"source_object_id":item.id,"source_scope_id":candidate.source_scope,"package_sha256":candidate.hash,
                "imported_base_source_seq":candidate.includes_history.then_some(item.versions.first().ok_or_else(invalid)?.source_seq),
                "imported_base_source_recorded_at":item.versions.first().ok_or_else(invalid)?.source_recorded_at}),
        )
        .await?;
        first_created.get_or_insert((object, event));
        let version_mapping=item.versions.iter().enumerate().map(|(local,version)|
            json!({"source_seq":version.source_seq,"local_seq":local,"source_recorded_at":version.source_recorded_at})).collect::<Vec<_>>();
        pages.push(
            json!({"source_object_id":item.id,"object_id":object,"document_id":document,"versions":version_mapping}),
        );
    }
    for edge in &graph.edges {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO flow_relations(id,scope_id,workspace_id,relation_type,source_object_id,target_object_id,position_key,properties,created_by)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            vec![Uuid::new_v4().into(),destination.scope_id.into(),destination.workspace_id.into(),edge.relation_type.clone().into(),
                plan.object(edge.source_object_id)?.into(),plan.object(edge.target_object_id)?.into(),edge.position_key.clone().into(),
                edge.properties.clone().into(),actor.into()])).await?;
    }
    for reference in &graph.references {
        let id = Uuid::new_v4();
        let target = *plan.forms.get(&reference.target_id).ok_or_else(invalid)?;
        let (workspace, _) = form_destination.ok_or_else(invalid)?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO flow_bridge_references(id,scope_id,workspace_id,source_object_id,target_type,target_id,display,idempotency_key,created_by,created_by_kind)
             VALUES($1,$2,$3,$4,'form',$5,$6,$7,$8,'user')",
            vec![id.into(),destination.scope_id.into(),workspace.into(),plan.object(reference.source_object_id)?.into(),
                target.into(),reference.display.clone().into(),format!("portable_graph_ref:{id}").into(),actor.into()])).await?;
    }
    let (object, event) = first_created.ok_or_else(invalid)?;
    let result = json!({"pages":pages,"forms":forms,"include_history":candidate.includes_history});
    super::scope_commands::record(
        &tx,
        destination.scope_id,
        actor,
        key,
        body,
        object,
        event,
        result.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use collab_core::page_inline::{InlineMark, InlineSpan};

    fn version(index: u16, text: &str, spans: Vec<InlineSpan>) -> Version {
        Version {
            source_seq: i64::from(index),
            source_recorded_at: None,
            block_keys: vec!["0".repeat(64)],
            title: "T".into(),
            blocks: vec![Block {
                source_cid: "b0".into(),
                parent_cid: None,
                preorder: 0,
                content: Content::Paragraph,
                text: text.into(),
                spans,
            }],
        }
    }

    fn spans(count: usize) -> Vec<InlineSpan> {
        (0..count)
            .map(|k| InlineSpan {
                start: 2 * k,
                end: 2 * k + 1,
                mark: InlineMark::Bold,
                href: None,
            })
            .collect()
    }

    fn graph_of(versions: Vec<Version>) -> Graph {
        Graph {
            pages: vec![Page {
                id: Uuid::new_v4(),
                parent: None,
                versions,
                assets: Vec::new(),
            }],
            forms: Vec::new(),
            edges: Vec::new(),
            references: Vec::new(),
        }
    }

    fn refusal_details(error: &ApiError) -> serde_json::Value {
        assert_eq!(error.kind(), crate::error::ApiErrorKind::LimitExceeded, "{error:?}");
        let ApiError::Typed { details, .. } = error else {
            panic!("a typed error");
        };
        details.clone().expect("details")
    }

    /// A-3: 256 versions that alternately set and clear a bold span over every two characters of
    /// one text make the replay apply 256 mark edits per version into the same text, which took
    /// 19.6 s in an unoptimized build and was not metered. The package is refused before any
    /// replay, naming the limit.
    #[test]
    fn a_history_that_toggles_marks_in_every_version_is_refused_before_any_replay() {
        let text = "a".repeat(512);
        let graph = graph_of(
            (0..256u16)
                .map(|i| version(i, &text, if i % 2 == 0 { spans(256) } else { Vec::new() }))
                .collect(),
        );
        let error = validate(&graph, true).expect_err("the replay work must be bounded");
        let details = refusal_details(&error);
        assert_eq!(details["limit_kind"], "portable_graph_replay_work");
        assert_eq!(details["limit"], MAX_REPLAY_MARK_EDITS);
        assert!(details["observed"].as_u64().unwrap() > MAX_REPLAY_MARK_EDITS);
    }

    /// The variant that replaces the text of every version, which leaves style anchors over the
    /// deleted text, is bounded by the same count.
    #[test]
    fn a_history_that_replaces_the_text_and_marks_in_every_version_is_refused() {
        let (even, odd) = ("a".repeat(512), "b".repeat(512));
        let graph = graph_of(
            (0..256u16)
                .map(|i| version(i, if i % 2 == 0 { &even } else { &odd }, spans(256)))
                .collect(),
        );
        let details = refusal_details(&validate(&graph, true).expect_err("bounded"));
        assert_eq!(details["limit_kind"], "portable_graph_replay_work");
    }

    /// The work counted is the work the replay does: every edit of every changed block, and the
    /// count of the boundary is exact.
    #[test]
    fn the_replay_work_budget_is_exact_at_its_boundary() {
        let at_limit = graph_of(vec![version(0, &"a".repeat(2), spans(1))]);
        assert_eq!(replay_mark_edits(&at_limit.pages[0]), 1);
        let count = usize::try_from(MAX_REPLAY_MARK_EDITS).unwrap();
        let text = "a".repeat(2 * count + 1);
        assert!(validate(&graph_of(vec![version(0, &text, spans(count))]), true).is_ok());
        let error = validate(&graph_of(vec![version(0, &text, spans(count + 1))]), true).expect_err("one over");
        assert_eq!(refusal_details(&error)["observed"], MAX_REPLAY_MARK_EDITS + 1);
    }

    /// An honest history is cheap: 256 versions that each change one word of a block with a few
    /// spans stay far under the budget and replay in well under a second, in any build.
    #[test]
    fn an_honest_history_is_within_the_budget_and_replays() {
        let versions = (0..256u16)
            .map(|i| {
                let text = format!("{}{}", "word ".repeat(8), i);
                version(i, &text, spans(4))
            })
            .collect::<Vec<_>>();
        let graph = graph_of(versions);
        validate(&graph, true).expect("an honest history is accepted");
        let edits = replay_mark_edits(&graph.pages[0]);
        assert!(edits < MAX_REPLAY_MARK_EDITS / 4, "{edits}");
        let plan = ImportPlan::new(&graph);
        let started = std::time::Instant::now();
        let frames = frames(&graph.pages[0], &plan).expect("an honest history replays");
        assert_eq!(frames.len(), 256);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
