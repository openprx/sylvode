//! Protocol 3 contributions: discovery, entry, read-only data, invoke and configuration reset.
use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DatabaseTransaction, FromQueryResult, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::Sha256;
use uuid::Uuid;

use super::{
    APPLY, ActivePackage, ContributionTarget, Execution, PREVIEW, RecordContext, admin, binding_receipt,
    contribution_unavailable, enabled, execute, insert_binding_receipt, is_v3, lock_admin, lock_binding, lock_project,
    read_allowed, record_context, require_extensions, stale_binding, statement, stored_layouts, stored_manifest_v3,
    supports_form, v3_record_target, write_allowed,
};
use crate::{
    error::{ApiError, ApiErrorKind},
    forms::{
        permissions::{append_record_scope_sql, denied_read_field_keys, form_action_allowed},
        schema::{FormField, parse_fields},
    },
    middleware::bot_auth::require_workspace_access_from_auth,
    plugins::{
        package::{Placement, bad, validate_key},
        package_v3::{
            ContributionV3, ManifestV3, Profile, READ_GRANT_PREFIX, SCHEMA_V3, Slot, UI_DISPLAY, plugin_error,
        },
    },
    response::ApiResponse,
    routes::form::{self, FormResponse},
};

const DATA_PAGE_LIMIT: i64 = 50;
const CURSOR_MAX_BYTES: usize = 512;
const BUCKET_CAPACITY: f64 = 30.0;
const BUCKET_WINDOW: Duration = Duration::from_secs(10);
const BUCKET_PRUNE_THRESHOLD: usize = 4096;

/// Field types a protocol 3 read may return. Media, people, relations and generated summaries
/// are never exposed to a plugin.
const READABLE_FIELD_TYPES: [&str; 20] = [
    "text",
    "textarea",
    "rich_text",
    "number",
    "integer",
    "amount",
    "rating",
    "progress",
    "date",
    "datetime",
    "single_select",
    "multi_select",
    "boolean",
    "autonumber",
    "formula",
    "email",
    "phone",
    "address",
    "location",
    "scan",
];

fn invalid(reason: &'static str, message: &str) -> ApiError {
    plugin_error(ApiErrorKind::InvalidUpdate, reason, message)
}

fn context_invalid() -> ApiError {
    invalid(
        "context_invalid",
        "the contribution context does not match this project",
    )
}

fn data_op_invalid() -> ApiError {
    invalid("data_op_invalid", "invalid plugin data request")
}

fn read_not_granted() -> ApiError {
    plugin_error(
        ApiErrorKind::Forbidden,
        "data_read_not_granted",
        "this plugin read has not been granted",
    )
}

pub(crate) fn actor(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::unauthenticated("invalid user identity"))
}

/// The project and the caller's role, after both feature switches.
pub(crate) struct Member {
    pub(crate) actor_id: Uuid,
    pub(crate) role: String,
}

pub(crate) async fn project_member(state: &AppState, claims: &JwtClaims, project_id: Uuid) -> Result<Member, ApiError> {
    enabled()?;
    require_extensions()?;
    let workspace_id: Uuid = state
        .db
        .query_one(statement(
            "SELECT workspace_id FROM projects WHERE id=$1",
            vec![project_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("project not found"))?
        .try_get("", "workspace_id")?;
    let (actor_id, role, _) = require_workspace_access_from_auth(state, claims, None, workspace_id).await?;
    Ok(Member { actor_id, role })
}

/// Where a discovery or entry request is asking from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface<'a> {
    Project,
    Form(&'a str),
    Record(&'a str),
}

impl Surface<'_> {
    fn admits(self, contribution: &ContributionV3) -> bool {
        let form_key = contribution.form_key.as_deref();
        match self {
            Self::Project => contribution.slot == Slot::ProjectView,
            Self::Form(key) => contribution.slot == Slot::FormListToolbar && form_key == Some(key),
            Self::Record(key) => {
                matches!(contribution.slot, Slot::FormRecordDetail | Slot::FormRecordActions) && form_key == Some(key)
            }
        }
    }
}

/// Context a contribution's entry requires, decided by its slot alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryContext {
    Project,
    Form(Uuid),
    Record(Uuid),
}

fn entry_context(slot: Slot, record_id: Option<Uuid>, form_id: Option<Uuid>) -> Result<EntryContext, ApiError> {
    match (slot, record_id, form_id) {
        (Slot::ProjectView, None, None) => Ok(EntryContext::Project),
        (Slot::FormListToolbar, None, Some(form_id)) => Ok(EntryContext::Form(form_id)),
        (Slot::FormRecordDetail | Slot::FormRecordActions, Some(record_id), None) => {
            Ok(EntryContext::Record(record_id))
        }
        _ => Err(context_invalid()),
    }
}

/// An empty query value is the same as an absent one; anything else must be a UUID.
fn query_uuid(value: Option<&str>) -> Result<Option<Uuid>, ApiError> {
    match value {
        None | Some("") => Ok(None),
        Some(value) => Uuid::parse_str(value).map(Some).map_err(|_| context_invalid()),
    }
}

/// Per user and contribution presentation preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct Preference {
    hidden: bool,
    collapsed: bool,
    position: Option<i16>,
}

const DEFAULT_PREFERENCE: Preference = Preference {
    hidden: false,
    collapsed: false,
    position: None,
};

#[derive(FromQueryResult)]
struct PreferenceRow {
    key: String,
    contribution_id: String,
    hidden: bool,
    collapsed: bool,
    position: Option<i16>,
}

async fn preferences<C: ConnectionTrait>(
    db: &C,
    actor_id: Uuid,
    project_id: Uuid,
    key: Option<&str>,
) -> Result<HashMap<(String, String), Preference>, ApiError> {
    Ok(PreferenceRow::find_by_statement(statement(
        "SELECT key,contribution_id,hidden,collapsed,position FROM form_plugin_user_preferences
          WHERE user_id=$1 AND project_id=$2 AND ($3::text IS NULL OR key=$3)",
        vec![actor_id.into(), project_id.into(), key.map(str::to_owned).into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| {
        (
            (row.key, row.contribution_id),
            Preference {
                hidden: row.hidden,
                collapsed: row.collapsed,
                position: row.position,
            },
        )
    })
    .collect())
}

/// Facts about the caller that decide which granted permissions actually apply.
pub(crate) struct GrantFacts<'a> {
    /// Keys of active project Forms the caller may view.
    pub(crate) readable_forms: &'a BTreeSet<String>,
    /// Form key of the record in context, for record contributions.
    pub(crate) record_form: Option<&'a str>,
    /// The record Form supports the amount profile and the caller may read its inputs.
    pub(crate) amount_readable: bool,
    /// The caller may also write the amount field.
    pub(crate) amount_writable: bool,
}

/// The permissions a caller actually holds through one contribution, sorted.
pub(crate) fn effective_grants(
    granted: &[String],
    manifest: &ManifestV3,
    contribution: &ContributionV3,
    facts: &GrantFacts<'_>,
) -> Vec<String> {
    let has = |grant: &str| granted.iter().any(|candidate| candidate == grant);
    let mut grants = vec![UI_DISPLAY.to_owned()];
    for read in &manifest.reads {
        let grant = format!("{READ_GRANT_PREFIX}{}", read.id);
        if has(&grant) && facts.readable_forms.contains(&read.form_key) {
            grants.push(grant);
        }
    }
    let amount_target = facts
        .record_form
        .is_some_and(|form_key| v3_record_target(manifest, &contribution.id, form_key));
    if amount_target && facts.amount_readable && has(PREVIEW) {
        grants.push(PREVIEW.to_owned());
        if facts.amount_writable && has(APPLY) {
            grants.push(APPLY.to_owned());
        }
    }
    grants.sort_unstable();
    grants
}

#[derive(FromQueryResult)]
struct ActiveV3 {
    package_id: Uuid,
    key: String,
    version: String,
    generation: i64,
    grants: Vec<String>,
    manifest: Value,
    package_sha256: String,
}

/// Bound protocol 3 packages whose binding grants `ui.display` and whose library link stands.
async fn active_packages(state: &AppState, project_id: Uuid, key: Option<&str>) -> Result<Vec<ActiveV3>, ApiError> {
    ActiveV3::find_by_statement(statement(
        "SELECT p.id AS package_id,p.key,p.version,b.generation,b.grants,p.manifest,p.package_sha256
           FROM form_plugin_bindings b JOIN form_plugin_packages p ON p.id=b.package_id
          WHERE b.project_id=$1 AND ($2::text IS NULL OR b.key=$2) AND p.library_revoked_at IS NULL
            AND p.manifest->>'schema_version'=$3 AND $4=ANY(b.grants)
          ORDER BY p.key",
        vec![
            project_id.into(),
            key.map(str::to_owned).into(),
            SCHEMA_V3.into(),
            UI_DISPLAY.into(),
        ],
    ))
    .all(&state.db)
    .await
    .map_err(ApiError::from)
}

#[derive(Serialize)]
pub(crate) struct ContributionItem {
    package_id: Uuid,
    key: String,
    version: String,
    name: String,
    profile: Profile,
    generation: i64,
    package_sha256: String,
    web_sha256: String,
    contribution: Value,
    project_position: u8,
    preference: Preference,
    effective_position: u8,
    grants: Vec<String>,
}

impl ContributionItem {
    fn order_key(&self) -> (u8, &str, &str) {
        let id = self.contribution.get("id").and_then(Value::as_str).unwrap_or_default();
        (self.effective_position, self.key.as_str(), id)
    }
}

fn sort_items(items: &mut [ContributionItem]) {
    items.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
}

fn effective_position(project_position: u8, preference: Preference) -> u8 {
    preference
        .position
        .and_then(|position| u8::try_from(position).ok())
        .unwrap_or(project_position)
}

/// Builds the visible items of one package for `surface`, optionally restricted to one contribution.
fn package_items(
    package: &ActiveV3,
    surface: Surface<'_>,
    only: Option<&str>,
    layouts: &HashMap<String, Vec<Placement>>,
    preferences: &HashMap<(String, String), Preference>,
    facts: &GrantFacts<'_>,
) -> Result<Vec<ContributionItem>, ApiError> {
    let manifest = stored_manifest_v3(&package.manifest)?;
    let placements = super::v3_layout(&manifest, layouts);
    let mut items = Vec::new();
    for contribution in &manifest.contributions {
        if only.is_some_and(|id| id != contribution.id) || !surface.admits(contribution) {
            continue;
        }
        let Some(placement) = placements
            .iter()
            .find(|placement| placement.id == contribution.id && placement.enabled)
        else {
            continue;
        };
        let preference = preferences
            .get(&(package.key.clone(), contribution.id.clone()))
            .copied()
            .unwrap_or(DEFAULT_PREFERENCE);
        items.push(ContributionItem {
            package_id: package.package_id,
            key: package.key.clone(),
            version: package.version.clone(),
            name: manifest.name.clone(),
            profile: manifest.profile,
            generation: package.generation,
            package_sha256: package.package_sha256.clone(),
            web_sha256: manifest.entry.sha256.clone(),
            contribution: serde_json::to_value(contribution).map_err(|_| ApiError::Internal)?,
            project_position: placement.position,
            preference,
            effective_position: effective_position(placement.position, preference),
            grants: effective_grants(&package.grants, &manifest, contribution, facts),
        });
    }
    Ok(items)
}

#[derive(FromQueryResult)]
struct FormKeyRow {
    id: Uuid,
    key: String,
}

/// Keys of the active project Forms that the reads of `packages` name and the caller may view.
async fn readable_forms(
    state: &AppState,
    project_id: Uuid,
    role: &str,
    packages: &[ActiveV3],
) -> Result<BTreeSet<String>, ApiError> {
    let mut wanted = BTreeSet::new();
    for package in packages {
        for read in stored_manifest_v3(&package.manifest)?.reads {
            wanted.insert(read.form_key);
        }
    }
    let mut readable = BTreeSet::new();
    if wanted.is_empty() {
        return Ok(readable);
    }
    let forms = FormKeyRow::find_by_statement(statement(
        "SELECT id,key FROM project_forms WHERE project_id=$1 AND archived_at IS NULL",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;
    for form in forms {
        if wanted.contains(&form.key) && form_action_allowed(state, form.id, role, "form.view").await? {
            readable.insert(form.key);
        }
    }
    Ok(readable)
}

/// Amount capability facts for a record, using the same checks as the protocol 2 discovery.
async fn amount_facts(state: &AppState, context: &RecordContext) -> Result<(bool, bool), ApiError> {
    if !supports_form(&context.form)? {
        return Ok((false, false));
    }
    match read_allowed(state, context).await {
        Ok(()) => {}
        Err(error) if error.kind() == ApiErrorKind::Forbidden => return Ok((false, false)),
        Err(error) => return Err(error),
    }
    match write_allowed(state, context).await {
        Ok(()) => Ok((true, true)),
        Err(error) if error.kind() == ApiErrorKind::Forbidden => Ok((true, false)),
        Err(error) => Err(error),
    }
}

/// Everything needed to answer discovery or entry for one caller and surface.
struct Lookup<'a> {
    state: &'a AppState,
    project_id: Uuid,
    actor_id: Uuid,
    role: &'a str,
    surface: Surface<'a>,
    record: Option<&'a RecordContext>,
}

impl Lookup<'_> {
    async fn items(&self, key: Option<&str>, only: Option<&str>) -> Result<Vec<ContributionItem>, ApiError> {
        let packages = active_packages(self.state, self.project_id, key).await?;
        if packages.is_empty() {
            return Ok(Vec::new());
        }
        let layouts = stored_layouts(&self.state.db, self.project_id, key).await?;
        let preferences = preferences(&self.state.db, self.actor_id, self.project_id, key).await?;
        let readable = readable_forms(self.state, self.project_id, self.role, &packages).await?;
        let (amount_readable, amount_writable) = match self.record {
            Some(context) => amount_facts(self.state, context).await?,
            None => (false, false),
        };
        let facts = GrantFacts {
            readable_forms: &readable,
            record_form: self.record.map(|context| context.form.key.as_str()),
            amount_readable,
            amount_writable,
        };
        let mut items = Vec::new();
        for package in &packages {
            items.extend(package_items(
                package,
                self.surface,
                only,
                &layouts,
                &preferences,
                &facts,
            )?);
        }
        sort_items(&mut items);
        Ok(items)
    }
}

/// An active, viewable Form of this project, or `context_invalid`.
async fn project_form(state: &AppState, project_id: Uuid, form_id: Uuid) -> Result<FormResponse, ApiError> {
    match form::find_form(state, form_id).await {
        Ok(form) if form.project_id == project_id && form.archived_at.is_none() => Ok(form),
        Ok(_) => Err(context_invalid()),
        Err(error) if error.kind() == ApiErrorKind::NotFound => Err(context_invalid()),
        Err(error) => Err(error),
    }
}

#[derive(Deserialize)]
pub struct DiscoveryQuery {
    surface: Option<String>,
    form_id: Option<String>,
}

pub async fn project_contributions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<DiscoveryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let member = project_member(&state, &claims, project_id).await?;
    let form_id = query_uuid(query.form_id.as_deref())?;
    let form = match (query.surface.as_deref(), form_id) {
        (Some("project"), None) => None,
        (Some("form"), Some(form_id)) => {
            let form = project_form(&state, project_id, form_id).await?;
            if !form_action_allowed(&state, form.id, &member.role, "form.view").await? {
                return Err(ApiError::forbidden("form permission denied: form.view"));
            }
            Some(form)
        }
        _ => return Err(context_invalid()),
    };
    let lookup = Lookup {
        state: &state,
        project_id,
        actor_id: member.actor_id,
        role: &member.role,
        surface: form.as_ref().map_or(Surface::Project, |form| Surface::Form(&form.key)),
        record: None,
    };
    Ok(ApiResponse::success(json!({"items": lookup.items(None, None).await?})))
}

pub async fn record_contributions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(record_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let context = record_context(&state, &claims, record_id).await?;
    require_extensions()?;
    let lookup = Lookup {
        state: &state,
        project_id: context.form.project_id,
        actor_id: context.actor_id,
        role: &context.role,
        surface: Surface::Record(&context.form.key),
        record: Some(&context),
    };
    Ok(ApiResponse::success(json!({"items": lookup.items(None, None).await?})))
}

#[derive(Deserialize)]
pub struct EntryQuery {
    record_id: Option<String>,
    form_id: Option<String>,
}

/// Visibility failures of the caller's own context read as an unavailable contribution.
fn hide_denial(error: ApiError) -> ApiError {
    if error.kind() == ApiErrorKind::Forbidden {
        contribution_unavailable()
    } else {
        error
    }
}

pub async fn entry(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, key, contribution_id)): Path<(Uuid, String, String)>,
    Query(query): Query<EntryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let member = project_member(&state, &claims, project_id)
        .await
        .map_err(|error| match error.kind() {
            ApiErrorKind::FeatureDisabled | ApiErrorKind::Unauthenticated => error,
            _ => contribution_unavailable(),
        })?;
    let record_id = query_uuid(query.record_id.as_deref())?;
    let form_id = query_uuid(query.form_id.as_deref())?;
    if validate_key(&key).is_err() || validate_key(&contribution_id).is_err() {
        return Err(contribution_unavailable());
    }
    let package = active_packages(&state, project_id, Some(&key))
        .await?
        .into_iter()
        .next()
        .ok_or_else(contribution_unavailable)?;
    let manifest = stored_manifest_v3(&package.manifest)?;
    let slot = manifest
        .contribution(&contribution_id)
        .ok_or_else(contribution_unavailable)?
        .slot;
    let mut form = None;
    let mut record = None;
    match entry_context(slot, record_id, form_id)? {
        EntryContext::Project => {}
        EntryContext::Form(form_id) => {
            let found = project_form(&state, project_id, form_id).await?;
            if !form_action_allowed(&state, found.id, &member.role, "form.view").await? {
                return Err(contribution_unavailable());
            }
            form = Some(found);
        }
        EntryContext::Record(record_id) => {
            record = Some(match record_context(&state, &claims, record_id).await {
                Ok(context) if context.form.project_id == project_id => context,
                Ok(_) => return Err(context_invalid()),
                Err(error) if error.kind() == ApiErrorKind::NotFound => return Err(context_invalid()),
                Err(error) => return Err(hide_denial(error)),
            });
        }
    }
    let surface = match (&form, &record) {
        (Some(form), _) => Surface::Form(&form.key),
        (None, Some(record)) => Surface::Record(&record.form.key),
        (None, None) => Surface::Project,
    };
    let lookup = Lookup {
        state: &state,
        project_id,
        actor_id: member.actor_id,
        role: &member.role,
        surface,
        record: record.as_ref(),
    };
    let item = lookup
        .items(Some(&key), Some(&contribution_id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(contribution_unavailable)?;
    let html: String = state
        .db
        .query_one(statement(
            "SELECT html FROM form_plugin_packages WHERE id=$1",
            vec![item.package_id.into()],
        ))
        .await?
        .ok_or_else(contribution_unavailable)?
        .try_get("", "html")?;
    Ok(ApiResponse::success(json!({"item": item, "html": html})))
}

/// One parsed data request; `op` decides which of the optional parts apply.
#[derive(Debug, PartialEq, Eq)]
struct DataRequest {
    package_id: Uuid,
    generation: i64,
    request_id: Uuid,
    read: String,
    op: DataOp,
}

#[derive(Debug, PartialEq, Eq)]
enum DataOp {
    Describe,
    Query { limit: i64, cursor: Option<String> },
    Get { record_id: Uuid },
}

impl DataOp {
    const fn name(&self) -> &'static str {
        match self {
            Self::Describe => "form.describe",
            Self::Query { .. } => "records.query",
            Self::Get { .. } => "record.get",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeBody {
    package_id: Uuid,
    generation: i64,
    request_id: Uuid,
    read: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryBody {
    package_id: Uuid,
    generation: i64,
    request_id: Uuid,
    read: String,
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetBody {
    package_id: Uuid,
    generation: i64,
    request_id: Uuid,
    read: String,
    record_id: Uuid,
}

fn parse_data_request(body: &[u8]) -> Result<DataRequest, ApiError> {
    let Ok(Value::Object(mut object)) = serde_json::from_slice::<Value>(body) else {
        return Err(data_op_invalid());
    };
    let Some(Value::String(op)) = object.remove("op") else {
        return Err(data_op_invalid());
    };
    let body = Value::Object(object);
    let request = match op.as_str() {
        "form.describe" => {
            let body = DescribeBody::deserialize(body).map_err(|_| data_op_invalid())?;
            DataRequest {
                package_id: body.package_id,
                generation: body.generation,
                request_id: body.request_id,
                read: body.read,
                op: DataOp::Describe,
            }
        }
        "records.query" => {
            let body = QueryBody::deserialize(body).map_err(|_| data_op_invalid())?;
            let limit = body.limit.unwrap_or(DATA_PAGE_LIMIT);
            if !(1..=DATA_PAGE_LIMIT).contains(&limit) {
                return Err(data_op_invalid());
            }
            DataRequest {
                package_id: body.package_id,
                generation: body.generation,
                request_id: body.request_id,
                read: body.read,
                op: DataOp::Query {
                    limit,
                    cursor: body.cursor,
                },
            }
        }
        "record.get" => {
            let body = GetBody::deserialize(body).map_err(|_| data_op_invalid())?;
            DataRequest {
                package_id: body.package_id,
                generation: body.generation,
                request_id: body.request_id,
                read: body.read,
                op: DataOp::Get {
                    record_id: body.record_id,
                },
            }
        }
        _ => return Err(data_op_invalid()),
    };
    if request.request_id.get_version_num() != 4 || request.read.is_empty() || request.read.len() > 32 {
        return Err(data_op_invalid());
    }
    Ok(request)
}

/// The cursor names the last row of a page. It is signed so a client cannot forge a position or
/// replay a cursor against another Form or read.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorPosition {
    t: i64,
    i: Uuid,
}

fn cursor_mac(secret: &str, scope: &str, payload: &str) -> Result<Hmac<Sha256>, ApiError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::Internal)?;
    mac.update(b"form-plugin-cursor:");
    mac.update(scope.as_bytes());
    mac.update(b":");
    mac.update(payload.as_bytes());
    Ok(mac)
}

fn encode_cursor(secret: &str, scope: &str, position: &CursorPosition) -> Result<String, ApiError> {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(position).map_err(|_| ApiError::Internal)?);
    let tag = cursor_mac(secret, scope, &payload)?.finalize().into_bytes();
    Ok(format!("{payload}.{}", URL_SAFE_NO_PAD.encode(tag)))
}

fn decode_cursor(secret: &str, scope: &str, cursor: &str) -> Result<CursorPosition, ApiError> {
    if cursor.len() > CURSOR_MAX_BYTES {
        return Err(data_op_invalid());
    }
    let (payload, tag) = cursor.split_once('.').ok_or_else(data_op_invalid)?;
    let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| data_op_invalid())?;
    cursor_mac(secret, scope, payload)?
        .verify_slice(&tag)
        .map_err(|_| data_op_invalid())?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| data_op_invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| data_op_invalid())
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    const fn full(now: Instant) -> Self {
        Self {
            tokens: BUCKET_CAPACITY,
            updated: now,
        }
    }

    /// Refills at capacity per window and spends one token when available.
    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = elapsed
            .mul_add(BUCKET_CAPACITY / BUCKET_WINDOW.as_secs_f64(), self.tokens)
            .min(BUCKET_CAPACITY);
        self.updated = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn idle(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.updated) >= BUCKET_WINDOW
    }
}

type BucketKey = (Uuid, Uuid, String);

static BUCKETS: LazyLock<Mutex<HashMap<BucketKey, Bucket>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn take_token(buckets: &mut HashMap<BucketKey, Bucket>, key: BucketKey, now: Instant) -> bool {
    if buckets.len() >= BUCKET_PRUNE_THRESHOLD {
        // An idle bucket has refilled completely, so dropping it changes no decision.
        buckets.retain(|_, bucket| !bucket.idle(now));
    }
    buckets.entry(key).or_insert_with(|| Bucket::full(now)).take(now)
}

fn rate_limit(actor_id: Uuid, project_id: Uuid, key: &str) -> Result<(), ApiError> {
    if take_token(
        &mut BUCKETS.lock(),
        (actor_id, project_id, key.to_owned()),
        Instant::now(),
    ) {
        Ok(())
    } else {
        Err(plugin_error(
            ApiErrorKind::LimitExceeded,
            "data_rate_limited",
            "plugin data requests are rate limited; retry shortly",
        ))
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct DescribedField {
    key: String,
    label: String,
    #[serde(rename = "type")]
    field_type: String,
}

/// Which declared fields a read returns, and why the others are withheld.
#[derive(Debug, PartialEq, Eq)]
struct FieldPlan {
    fields: Vec<DescribedField>,
    missing: Vec<String>,
    denied: Vec<String>,
}

fn plan_fields(declared: &[String], schema: &[FormField], denied_by_policy: &BTreeSet<String>) -> FieldPlan {
    let mut plan = FieldPlan {
        fields: Vec::new(),
        missing: Vec::new(),
        denied: Vec::new(),
    };
    for key in declared {
        match schema.iter().find(|field| &field.key == key) {
            None => plan.missing.push(key.clone()),
            Some(field)
                if denied_by_policy.contains(key) || !READABLE_FIELD_TYPES.contains(&field.field_type.as_str()) =>
            {
                plan.denied.push(key.clone());
            }
            Some(field) => plan.fields.push(DescribedField {
                key: field.key.clone(),
                label: field.label.clone(),
                field_type: field.field_type.clone(),
            }),
        }
    }
    plan
}

/// Copies only the planned fields; titles and other fields never leave the server.
fn project_values(values: &Value, fields: &[DescribedField]) -> Value {
    let mut projected = Map::new();
    for field in fields {
        if let Some(value) = values.get(&field.key) {
            projected.insert(field.key.clone(), value.clone());
        }
    }
    Value::Object(projected)
}

#[derive(FromQueryResult)]
struct DataRow {
    id: Uuid,
    values: Value,
    created_at: Timestamp,
    updated_at: Timestamp,
    created_us: i64,
}

fn data_item(row: &DataRow, fields: &[DescribedField]) -> Value {
    json!({
        "id": row.id,
        "values": project_values(&row.values, fields),
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    })
}

/// Membership, binding and visibility checks of a data read, under the row locks that revocation
/// paths take exclusively. Returns the parsed manifest.
async fn lock_data_binding(
    tx: &DatabaseTransaction,
    project_id: Uuid,
    workspace_id: Uuid,
    actor_id: Uuid,
    key: &str,
    contribution_id: &str,
    request: &DataRequest,
) -> Result<(String, ManifestV3, Vec<String>), ApiError> {
    let role: String = tx
        .query_one(statement(
            "SELECT role FROM workspace_members WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
            vec![workspace_id.into(), actor_id.into()],
        ))
        .await?
        .ok_or_else(contribution_unavailable)?
        .try_get("", "role")?;
    let binding = tx
        .query_one(statement(
            "SELECT b.package_id,b.generation,b.grants,p.manifest,p.library_revoked_at IS NULL AS linked
               FROM form_plugin_bindings b LEFT JOIN form_plugin_packages p ON p.id=b.package_id
              WHERE b.project_id=$1 AND b.key=$2 FOR SHARE OF b",
            vec![project_id.into(), key.into()],
        ))
        .await?
        .ok_or_else(contribution_unavailable)?;
    if binding.try_get::<Option<Uuid>>("", "package_id")? != Some(request.package_id)
        || binding.try_get::<i64>("", "generation")? != request.generation
    {
        return Err(stale_binding());
    }
    let grants: Vec<String> = binding.try_get("", "grants")?;
    let manifest: Value = binding.try_get("", "manifest")?;
    if !binding.try_get::<bool>("", "linked")? || !is_v3(&manifest) || !grants.iter().any(|grant| grant == UI_DISPLAY) {
        return Err(contribution_unavailable());
    }
    let manifest = stored_manifest_v3(&manifest)?;
    let layouts = stored_layouts(tx, project_id, Some(key)).await?;
    if manifest.contribution(contribution_id).is_none()
        || !super::v3_layout(&manifest, &layouts)
            .iter()
            .any(|placement| placement.id == contribution_id && placement.enabled)
    {
        return Err(contribution_unavailable());
    }
    Ok((role, manifest, grants))
}

pub async fn data(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, key, contribution_id)): Path<(Uuid, String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    require_extensions()?;
    let request = parse_data_request(&body)?;
    let actor_id = actor(&claims)?;
    if validate_key(&key).is_err() || validate_key(&contribution_id).is_err() {
        return Err(contribution_unavailable());
    }
    rate_limit(actor_id, project_id, &key)?;
    let workspace_id: Uuid = state
        .db
        .query_one(statement(
            "SELECT workspace_id FROM projects WHERE id=$1",
            vec![project_id.into()],
        ))
        .await?
        .ok_or_else(contribution_unavailable)?
        .try_get("", "workspace_id")?;
    let tx = state.db.begin().await?;
    let (role, manifest, grants) = lock_data_binding(
        &tx,
        project_id,
        workspace_id,
        actor_id,
        &key,
        &contribution_id,
        &request,
    )
    .await?;
    let read = manifest.read(&request.read).ok_or_else(read_not_granted)?;
    if !grants
        .iter()
        .any(|grant| grant.strip_prefix(READ_GRANT_PREFIX) == Some(read.id.as_str()))
    {
        return Err(read_not_granted());
    }
    let form = form::find_form_with_conn(&tx, {
        let row = tx
            .query_one(statement(
                "SELECT id FROM project_forms WHERE project_id=$1 AND key=$2 AND archived_at IS NULL",
                vec![project_id.into(), read.form_key.clone().into()],
            ))
            .await?
            .ok_or_else(read_not_granted)?;
        row.try_get::<Uuid>("", "id")?
    })
    .await?;
    // Shared with every reader and exclusive for Form permission writers: a permission change
    // either commits before this check or waits until this read has finished.
    tx.execute(statement(
        "SELECT pg_advisory_xact_lock_shared(hashtextextended($1, 19019))",
        vec![form.id.to_string().into()],
    ))
    .await?;
    if !form_action_allowed(&state, form.id, &role, "form.view").await? {
        return Err(read_not_granted());
    }
    let schema = parse_fields(&form.schema).map_err(|_| ApiError::Internal)?;
    let denied = denied_read_field_keys(&state, form.id, &role).await?;
    let plan = plan_fields(&read.fields, &schema, &denied);
    let response = match &request.op {
        DataOp::Describe => json!({
            "read": read.id,
            "form": {"key": form.key, "name": form.name, "fields": plan.fields},
            "missing_fields": plan.missing,
            "denied_fields": plan.denied,
        }),
        DataOp::Query { limit, cursor } => {
            let scope = format!("{}:{}", form.id, read.id);
            let secret = state.cfg.jwt_secret.expose();
            let after = cursor
                .as_deref()
                .map(|cursor| decode_cursor(secret, &scope, cursor))
                .transpose()?;
            let mut where_parts = vec!["form_id = $1".to_owned(), "archived_at IS NULL".to_owned()];
            let mut values: Vec<sea_orm::Value> = vec![form.id.into()];
            let mut next_idx = 2;
            append_record_scope_sql(
                &state,
                form.id,
                &role,
                actor_id,
                &mut where_parts,
                &mut values,
                &mut next_idx,
            )
            .await?;
            if let Some(after) = &after {
                where_parts.push(format!(
                    "(({CREATED_US} < ${next_idx}) OR ({CREATED_US} = ${next_idx} AND id < ${}))",
                    next_idx + 1
                ));
                values.push(after.t.into());
                values.push(after.i.into());
                next_idx += 2;
            }
            values.push((limit + 1).into());
            let rows = DataRow::find_by_statement(statement(
                &format!(
                    "SELECT id,values,created_at,updated_at,{CREATED_US} AS created_us FROM form_records
                      WHERE {} ORDER BY created_at DESC,id DESC LIMIT ${next_idx}",
                    where_parts.join(" AND ")
                ),
                values,
            ))
            .all(&tx)
            .await?;
            let page = usize::try_from(*limit).map_err(|_| ApiError::Internal)?;
            let next_cursor = match (rows.len() > page, rows.get(page.saturating_sub(1))) {
                (true, Some(last)) => Some(encode_cursor(
                    secret,
                    &scope,
                    &CursorPosition {
                        t: last.created_us,
                        i: last.id,
                    },
                )?),
                _ => None,
            };
            let items: Vec<Value> = rows.iter().take(page).map(|row| data_item(row, &plan.fields)).collect();
            json!({"read": read.id, "items": items, "next_cursor": next_cursor})
        }
        DataOp::Get { record_id } => {
            let mut where_parts = vec![
                "id = $1".to_owned(),
                "form_id = $2".to_owned(),
                "archived_at IS NULL".to_owned(),
            ];
            let mut values: Vec<sea_orm::Value> = vec![(*record_id).into(), form.id.into()];
            let mut next_idx = 3;
            append_record_scope_sql(
                &state,
                form.id,
                &role,
                actor_id,
                &mut where_parts,
                &mut values,
                &mut next_idx,
            )
            .await?;
            let row = DataRow::find_by_statement(statement(
                &format!(
                    "SELECT id,values,created_at,updated_at,{CREATED_US} AS created_us FROM form_records WHERE {}",
                    where_parts.join(" AND ")
                ),
                values,
            ))
            .one(&tx)
            .await?
            .ok_or_else(|| {
                plugin_error(
                    ApiErrorKind::NotFound,
                    "record_unavailable",
                    "record is not available to this plugin read",
                )
            })?;
            json!({"read": read.id, "item": data_item(&row, &plan.fields)})
        }
    };
    tx.commit().await?;
    tracing::info!(
        actor_id = %actor_id,
        project_id = %project_id,
        key = %key,
        contribution = %contribution_id,
        op = request.op.name(),
        read = %read.id,
        request_id = %request.request_id,
        "form plugin data read"
    );
    Ok(ApiResponse::success(response))
}

/// Exact microsecond position of a record, used for cursor comparisons.
const CREATED_US: &str = "(EXTRACT(EPOCH FROM created_at) * 1000000)::bigint";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeRequest {
    package_id: Uuid,
    generation: i64,
    request_id: Uuid,
    preview_request_id: Option<Uuid>,
    capability: String,
    input: Value,
}

pub async fn invoke(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((record_id, key, contribution_id)): Path<(Uuid, String, String)>,
    Json(request): Json<InvokeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if request.input.as_object().is_none_or(|input| !input.is_empty())
        || request.request_id.is_nil()
        || !matches!(request.capability.as_str(), PREVIEW | APPLY)
        || (request.capability == APPLY) != request.preview_request_id.is_some()
    {
        return Err(bad(
            "invalid plugin capability, request ID, preview reference, or input",
        ));
    }
    let context = record_context(&state, &claims, record_id).await?;
    require_extensions()?;
    if validate_key(&key).is_err() || validate_key(&contribution_id).is_err() {
        return Err(contribution_unavailable());
    }
    read_allowed(&state, &context).await?;
    if request.capability == APPLY {
        write_allowed(&state, &context).await?;
    }
    if !supports_form(&context.form)? {
        return Err(ApiError::not_found("plugin package does not support this Form"));
    }
    let manifest: Value = state
        .db
        .query_one(statement(
            "SELECT manifest FROM form_plugin_packages
              WHERE project_id=$1 AND id=$2 AND key=$3 AND library_revoked_at IS NULL
                AND manifest->>'schema_version'=$4",
            vec![
                context.form.project_id.into(),
                request.package_id.into(),
                key.clone().into(),
                SCHEMA_V3.into(),
            ],
        ))
        .await?
        .ok_or_else(contribution_unavailable)?
        .try_get("", "manifest")?;
    if !v3_record_target(&stored_manifest_v3(&manifest)?, &contribution_id, &context.form.key) {
        return Err(contribution_unavailable());
    }
    let active = ActivePackage::find_by_statement(statement(
        "SELECT key,version,wasm_bytes,manifest->'wasm'->>'sha256' AS wasm_sha256
           FROM form_plugin_packages WHERE id=$1",
        vec![request.package_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(contribution_unavailable)?;
    let execution = Execution {
        workspace_id: context.form.workspace_id,
        project_id: context.form.project_id,
        key,
        package_id: request.package_id,
        generation: request.generation,
        actor_id: context.actor_id,
        role: context.role.clone(),
        form_id: context.form.id,
        record_id,
        created_by: context.record.created_by,
        record_updated_at: context.record.updated_at,
        schema_version: context.form.schema_version,
        capability: request.capability,
        request_id: request.request_id,
        preview_request_id: request.preview_request_id,
        contribution: Some(ContributionTarget {
            id: contribution_id,
            form_key: context.form.key.clone(),
        }),
    };
    Ok(ApiResponse::success(execute(&state, context, execution, active).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetRequest {
    expected_generation: i64,
    request_id: Uuid,
}

pub async fn reset_configuration(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, key)): Path<(Uuid, String)>,
    Json(request): Json<ResetRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace_id, actor_id) = admin(&state, &claims, project_id).await?;
    require_extensions()?;
    validate_key(&key)?;
    if request.request_id.is_nil() {
        return Err(bad("invalid binding request ID"));
    }
    let tx = state.db.begin().await?;
    lock_admin(&tx, workspace_id, actor_id).await?;
    lock_project(&tx, project_id).await?;
    let current = lock_binding(&tx, project_id, &key).await?;
    let payload = json!({
        "reason": "configuration_reset",
        "expected_generation": request.expected_generation,
        "request_id": request.request_id,
    });
    if let Some(response) = binding_receipt(&tx, project_id, &key, request.request_id, actor_id, &payload).await? {
        tx.commit().await?;
        return Ok(ApiResponse::success(response));
    }
    if current.generation != request.expected_generation {
        return Err(stale_binding());
    }
    if current.package_id.is_some() {
        return Err(invalid(
            "binding_active",
            "disable the plugin before clearing its configuration",
        ));
    }
    for table_statement in [
        "DELETE FROM form_plugin_contribution_layouts WHERE project_id=$1 AND key=$2",
        "DELETE FROM form_plugin_user_preferences WHERE project_id=$1 AND key=$2",
    ] {
        tx.execute(statement(table_statement, vec![project_id.into(), key.clone().into()]))
            .await?;
    }
    tx.execute(statement(
        "UPDATE form_plugin_bindings SET generation=generation+1 WHERE project_id=$1 AND key=$2",
        vec![project_id.into(), key.clone().into()],
    ))
    .await?;
    let binding = lock_binding(&tx, project_id, &key).await?;
    let response = serde_json::to_value(binding).map_err(|_| ApiError::Internal)?;
    insert_binding_receipt(&tx, project_id, &key, request.request_id, actor_id, payload, &response).await?;
    tx.commit().await?;
    Ok(ApiResponse::success(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::package_v3::{ContributionV3, Kind};

    fn manifest(profile: &str) -> ManifestV3 {
        let wasm = if profile == "wasm" {
            json!({"abi":"openpr.plugin.v1","path":"plugin.wasm","size":10,"sha256":"0".repeat(64),"form_key":"order_line"})
        } else {
            Value::Null
        };
        serde_json::from_value(json!({
            "schema_version": "openpr.plugin.v3", "key": "order_board", "name": "Order board",
            "version": "1.0.0", "publisher": "Author", "host_protocol": {"min": 3, "max": 3},
            "profile": profile, "bridge_version": 2,
            "entry": {"path": "panel.html", "size": 10, "sha256": "0".repeat(64)},
            "wasm": wasm,
            "contributions": [
                {"id":"board","kind":"view","slot":"project.view","label":"Board","view":"board","route":"board","form_key":null},
                {"id":"detail","kind":"panel","slot":"form.record.detail","label":"Detail","view":"d","route":null,"form_key":"order_line"},
                {"id":"tool","kind":"action","slot":"form.list.toolbar","label":"Tool","view":"t","route":null,"form_key":"order_line"},
                {"id":"other","kind":"panel","slot":"form.record.actions","label":"Other","view":"o","route":null,"form_key":"other_form"}
            ],
            "reads": [
                {"id":"orders","form_key":"order_line","fields":["quantity"]},
                {"id":"people","form_key":"people","fields":["name"]}
            ],
            "capabilities": []
        }))
        .unwrap_or_else(|error| panic!("test manifest: {error}"))
    }

    fn contribution<'a>(manifest: &'a ManifestV3, id: &str) -> &'a ContributionV3 {
        manifest
            .contribution(id)
            .unwrap_or_else(|| panic!("missing contribution {id}"))
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn grants_follow_binding_and_form_visibility() {
        let ui = manifest("ui");
        let readable = BTreeSet::from(["order_line".to_owned()]);
        let facts = GrantFacts {
            readable_forms: &readable,
            record_form: None,
            amount_readable: true,
            amount_writable: true,
        };
        let granted = strings(&["read:orders", "read:people", "ui.display"]);
        assert_eq!(
            effective_grants(&granted, &ui, contribution(&ui, "board"), &facts),
            strings(&["read:orders", "ui.display"])
        );
        assert_eq!(
            effective_grants(&strings(&["ui.display"]), &ui, contribution(&ui, "board"), &facts),
            strings(&["ui.display"])
        );
    }

    #[test]
    fn amount_grants_need_the_record_target_and_permissions() {
        let wasm = manifest("wasm");
        let none = BTreeSet::new();
        let granted = strings(&["amount.apply", "amount.preview", "ui.display"]);
        let mut facts = GrantFacts {
            readable_forms: &none,
            record_form: Some("order_line"),
            amount_readable: true,
            amount_writable: true,
        };
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "detail"), &facts),
            strings(&["amount.apply", "amount.preview", "ui.display"])
        );
        // Views and toolbars never carry capabilities.
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "board"), &facts),
            strings(&["ui.display"])
        );
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "tool"), &facts),
            strings(&["ui.display"])
        );
        // A record contribution for another Form does not target the WASM Form.
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "other"), &facts),
            strings(&["ui.display"])
        );
        facts.amount_writable = false;
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "detail"), &facts),
            strings(&["amount.preview", "ui.display"])
        );
        facts.amount_readable = false;
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "detail"), &facts),
            strings(&["ui.display"])
        );
        facts.amount_readable = true;
        facts.record_form = Some("other_form");
        assert_eq!(
            effective_grants(&granted, &wasm, contribution(&wasm, "detail"), &facts),
            strings(&["ui.display"])
        );
        facts.record_form = Some("order_line");
        assert_eq!(
            effective_grants(
                &strings(&["amount.apply", "ui.display"]),
                &wasm,
                contribution(&wasm, "detail"),
                &facts
            ),
            strings(&["ui.display"])
        );
        let ui = manifest("ui");
        assert_eq!(
            effective_grants(&granted, &ui, contribution(&ui, "detail"), &facts),
            strings(&["ui.display"])
        );
    }

    #[test]
    fn entry_context_matches_the_slot() {
        let id = Uuid::new_v4();
        assert_eq!(
            entry_context(Slot::ProjectView, None, None).ok(),
            Some(EntryContext::Project)
        );
        assert_eq!(
            entry_context(Slot::FormListToolbar, None, Some(id)).ok(),
            Some(EntryContext::Form(id))
        );
        assert_eq!(
            entry_context(Slot::FormRecordDetail, Some(id), None).ok(),
            Some(EntryContext::Record(id))
        );
        assert_eq!(
            entry_context(Slot::FormRecordActions, Some(id), None).ok(),
            Some(EntryContext::Record(id))
        );
        for (slot, record, form) in [
            (Slot::ProjectView, Some(id), None),
            (Slot::ProjectView, None, Some(id)),
            (Slot::FormListToolbar, None, None),
            (Slot::FormListToolbar, Some(id), Some(id)),
            (Slot::FormRecordDetail, None, None),
            (Slot::FormRecordDetail, Some(id), Some(id)),
            (Slot::FormRecordActions, None, Some(id)),
        ] {
            let error = entry_context(slot, record, form).err();
            assert!(error.is_some_and(|error| error.kind() == ApiErrorKind::InvalidUpdate));
        }
        assert_eq!(query_uuid(Some("")).ok(), Some(None));
        assert_eq!(query_uuid(None).ok(), Some(None));
        assert!(query_uuid(Some("not-a-uuid")).is_err());
    }

    #[test]
    fn surfaces_admit_only_their_slots() {
        let ui = manifest("ui");
        assert!(Surface::Project.admits(contribution(&ui, "board")));
        assert!(!Surface::Project.admits(contribution(&ui, "tool")));
        assert!(Surface::Form("order_line").admits(contribution(&ui, "tool")));
        assert!(!Surface::Form("people").admits(contribution(&ui, "tool")));
        assert!(!Surface::Form("order_line").admits(contribution(&ui, "detail")));
        assert!(Surface::Record("order_line").admits(contribution(&ui, "detail")));
        assert!(!Surface::Record("order_line").admits(contribution(&ui, "other")));
        assert!(!Surface::Record("order_line").admits(contribution(&ui, "board")));
        let made = ContributionV3 {
            id: "x".into(),
            kind: Kind::Action,
            slot: Slot::FormRecordActions,
            label: "X".into(),
            view: "x".into(),
            route: None,
            form_key: Some("other_form".into()),
        };
        assert!(Surface::Record("other_form").admits(&made));
    }

    #[test]
    fn cursor_round_trips_and_rejects_tampering() {
        let position = CursorPosition {
            t: 1_763_500_000_123_456,
            i: Uuid::new_v4(),
        };
        let cursor = encode_cursor("secret", "form:orders", &position).unwrap_or_default();
        assert_eq!(decode_cursor("secret", "form:orders", &cursor).ok(), Some(position));
        let rejected = |cursor: &str, scope: &str, secret: &str| {
            decode_cursor(secret, scope, cursor)
                .err()
                .is_some_and(|error| error.kind() == ApiErrorKind::InvalidUpdate)
        };
        assert!(rejected(&cursor, "form:other", "secret"));
        assert!(rejected(&cursor, "form:orders", "another-secret"));
        let (payload, tag) = cursor.split_once('.').unwrap_or_default();
        let forged = URL_SAFE_NO_PAD.encode(br#"{"t":1,"i":"00000000-0000-0000-0000-000000000000"}"#);
        assert!(rejected(&format!("{forged}.{tag}"), "form:orders", "secret"));
        assert!(rejected(payload, "form:orders", "secret"));
        assert!(rejected(&format!("{payload}.!!"), "form:orders", "secret"));
        assert!(rejected("", "form:orders", "secret"));
        assert!(rejected(&"a".repeat(CURSOR_MAX_BYTES + 1), "form:orders", "secret"));
    }

    #[test]
    fn token_bucket_allows_thirty_per_window() {
        let start = Instant::now();
        let mut bucket = Bucket::full(start);
        for _ in 0..30 {
            assert!(bucket.take(start));
        }
        assert!(!bucket.take(start));
        // One token returns every third of a second.
        assert!(!bucket.take(start + Duration::from_millis(300)));
        assert!(bucket.take(start + Duration::from_millis(334)));
        assert!(!bucket.take(start + Duration::from_millis(334)));
        // A long idle period refills to capacity and no further.
        let later = start + Duration::from_mins(1);
        for _ in 0..30 {
            assert!(bucket.take(later));
        }
        assert!(!bucket.take(later));
    }

    #[test]
    fn token_buckets_are_keyed_and_pruned_only_when_idle() {
        let start = Instant::now();
        let mut buckets = HashMap::new();
        let project = Uuid::new_v4();
        let first = (Uuid::new_v4(), project, "order_board".to_owned());
        for _ in 0..30 {
            assert!(take_token(&mut buckets, first.clone(), start));
        }
        assert!(!take_token(&mut buckets, first.clone(), start));
        assert!(take_token(
            &mut buckets,
            (Uuid::new_v4(), project, "order_board".to_owned()),
            start
        ));
        assert!(take_token(&mut buckets, (first.0, project, "other".to_owned()), start));
        for _ in 0..BUCKET_PRUNE_THRESHOLD {
            buckets.insert((Uuid::new_v4(), project, String::new()), Bucket::full(start));
        }
        // The exhausted bucket is not idle yet, so pruning keeps it and its decision.
        assert!(!take_token(&mut buckets, first, start + Duration::from_millis(100)));
        let later = start + Duration::from_secs(11);
        assert!(take_token(
            &mut buckets,
            (Uuid::new_v4(), project, "x".to_owned()),
            later
        ));
        assert!(buckets.len() < BUCKET_PRUNE_THRESHOLD);
    }

    #[test]
    fn data_requests_parse_strictly() {
        let request_id = Uuid::new_v4();
        let package_id = Uuid::new_v4();
        let base = json!({"package_id": package_id, "generation": 7, "request_id": request_id, "read": "orders"});
        let with = |extra: Value| {
            let mut body = base.clone();
            if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
                body.extend(extra.clone());
            }
            serde_json::to_vec(&body).unwrap_or_default()
        };
        assert_eq!(
            parse_data_request(&with(json!({"op": "form.describe"}))).ok(),
            Some(DataRequest {
                package_id,
                generation: 7,
                request_id,
                read: "orders".into(),
                op: DataOp::Describe
            })
        );
        assert_eq!(
            parse_data_request(&with(json!({"op": "records.query"})))
                .ok()
                .map(|request| request.op),
            Some(DataOp::Query {
                limit: 50,
                cursor: None
            })
        );
        assert_eq!(
            parse_data_request(&with(json!({"op": "records.query", "limit": null, "cursor": "c"})))
                .ok()
                .map(|request| request.op),
            Some(DataOp::Query {
                limit: 50,
                cursor: Some("c".into())
            })
        );
        let record_id = Uuid::new_v4();
        assert_eq!(
            parse_data_request(&with(json!({"op": "record.get", "record_id": record_id})))
                .ok()
                .map(|request| request.op),
            Some(DataOp::Get { record_id })
        );
        for invalid_body in [
            with(json!({"op": "records.query", "limit": 0})),
            with(json!({"op": "records.query", "limit": 51})),
            with(json!({"op": "form.describe", "limit": 5})),
            with(json!({"op": "record.get"})),
            with(json!({"op": "records.delete"})),
            with(json!({"op": "form.describe", "extra": true})),
            with(json!({"op": "form.describe", "request_id": Uuid::nil()})),
            with(json!({"op": "form.describe", "read": ""})),
            with(json!({})),
            b"[]".to_vec(),
            b"not json".to_vec(),
        ] {
            let error = parse_data_request(&invalid_body).err();
            assert!(error.is_some_and(|error| error.kind() == ApiErrorKind::InvalidUpdate));
        }
    }

    fn field(key: &str, field_type: &str) -> FormField {
        FormField {
            field_id: key.to_owned(),
            key: key.to_owned(),
            label: key.to_uppercase(),
            field_type: field_type.to_owned(),
            required: false,
            options: Vec::new(),
            option_disabled: Vec::new(),
            option_archived: Vec::new(),
            conditional: None,
            amount: None,
            signature: None,
        }
    }

    #[test]
    fn field_plans_withhold_denied_missing_and_unsafe_fields() {
        let schema = [
            field("quantity", "number"),
            field("unit_price", "number"),
            field("photo", "image"),
            field("note", "text"),
        ];
        let declared = strings(&["quantity", "unit_price", "photo", "gone"]);
        let plan = plan_fields(&declared, &schema, &BTreeSet::from(["unit_price".to_owned()]));
        assert_eq!(
            plan.fields,
            vec![DescribedField {
                key: "quantity".into(),
                label: "QUANTITY".into(),
                field_type: "number".into()
            }]
        );
        assert_eq!(plan.missing, strings(&["gone"]));
        assert_eq!(plan.denied, strings(&["unit_price", "photo"]));
        let values = json!({"quantity": {"type":"decimal","decimal":"2"}, "unit_price": "1", "note": "x"});
        assert_eq!(
            project_values(&values, &plan.fields),
            json!({"quantity": {"type":"decimal","decimal":"2"}})
        );
    }

    #[test]
    fn items_sort_by_position_key_and_id() {
        let item = |position: u8, key: &str, id: &str| ContributionItem {
            package_id: Uuid::nil(),
            key: key.into(),
            version: "1.0.0".into(),
            name: "N".into(),
            profile: Profile::Ui,
            generation: 1,
            package_sha256: String::new(),
            web_sha256: String::new(),
            contribution: json!({"id": id}),
            project_position: position,
            preference: DEFAULT_PREFERENCE,
            effective_position: position,
            grants: Vec::new(),
        };
        let mut items = vec![
            item(50, "b", "a"),
            item(50, "a", "z"),
            item(10, "z", "z"),
            item(50, "a", "b"),
        ];
        sort_items(&mut items);
        let order: Vec<_> = items
            .iter()
            .map(|item| {
                let (position, key, id) = item.order_key();
                format!("{position}:{key}:{id}")
            })
            .collect();
        assert_eq!(order, ["10:z:z", "50:a:b", "50:a:z", "50:b:a"]);
        let personal = Preference {
            position: Some(5),
            ..DEFAULT_PREFERENCE
        };
        assert_eq!(effective_position(50, personal), 5);
        assert_eq!(effective_position(50, DEFAULT_PREFERENCE), 50);
    }
}
