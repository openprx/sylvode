//! Handlers of the draft routes: agents write drafts, people review them.
//!
//! Creating and reading are open to bots with access to the project; approving and rejecting
//! are for a person who owns or administers the workspace, on a route no bot token or connected
//! application can reach. Approval saves through the ordinary path of the draft's kind with the
//! approver's rights, in one transaction with the status change.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    apply,
    prepare::{
        self, FormTarget, KIND_FORM_SCHEMA, KIND_LAYOUT, KIND_PACKAGE_UPLOAD, KIND_SITE_PAGE, LayoutTarget,
        MAX_SPEC_BYTES, PackageUploadTarget, Prepared, public_exposure, review_digest,
    },
    store::{self, DraftRow, ListFilter, NewDraft},
};
use crate::{
    error::ApiError,
    forms::permissions::{normalize_permission_subject_id, role_is_form_admin},
    layouts::{
        access::{Principal, Scope, ScopeKind, member, require_read, resolve_project, resolve_scope},
        apply::draft_slot,
    },
    middleware::{bot_auth::BotAuthContext, scoped_bot::caller_projects},
    packages::handlers::PreparedUpload,
    response::{ApiResponse, PageQuery, PaginatedData},
    routes::form::{find_form, require_form_action},
    site::drafts::PageTarget,
};

/// Creations of one author per minute, across projects.
const CREATIONS_PER_MINUTE: i64 = 30;
/// Pending drafts of one author in one project.
const PENDING_PER_AUTHOR: i64 = 50;
/// Pending package upload drafts of one workspace, all bots together: each keeps up to 32 MiB.
const PENDING_UPLOADS_PER_WORKSPACE: i64 = 10;
/// Pending package upload drafts of one bot, so one bot cannot take every place of the workspace.
const PENDING_UPLOADS_PER_BOT: i64 = 3;
const MAX_COMMENT_CHARS: usize = 2000;

fn rate_limited(message: &'static str) -> ApiError {
    ApiError::Coded {
        status: 429,
        code: "rate_limited",
        message: message.to_owned(),
        details: None,
    }
}

fn draft_not_found() -> ApiError {
    ApiError::not_found("draft not found")
}

/// A caller outside the workspace learns nothing about its drafts.
fn reads_as_not_found(error: ApiError) -> ApiError {
    match error {
        ApiError::Forbidden(_) | ApiError::NotFound(_) => draft_not_found(),
        other => other,
    }
}

/// The form whose `form.view` guards a draft: the form a `form_schema` draft changes, or the form a
/// `layout` draft is placed on. `None` for drafts that name no existing form. A stored target that
/// does not name its form properly is refused as not found (fail closed).
fn guarded_form(row: &DraftRow) -> Result<Option<Uuid>, ApiError> {
    let id_of = |field: &str| -> Result<Uuid, ApiError> {
        row.target
            .get(field)
            .and_then(Value::as_str)
            .and_then(|id| id.parse().ok())
            .ok_or_else(draft_not_found)
    };
    match row.kind.as_str() {
        KIND_FORM_SCHEMA if row.target.get("form_id").is_some_and(|id| !id.is_null()) => id_of("form_id").map(Some),
        KIND_LAYOUT if row.target.get("scope_kind").and_then(Value::as_str) == Some("form") => {
            id_of("scope_id").map(Some)
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Serialize)]
pub struct Author {
    kind: String,
    id: Uuid,
    name: Option<String>,
}

/// `DraftSummary` of contract C3 section 7.
#[derive(Debug, Serialize)]
pub struct DraftSummary {
    id: Uuid,
    kind: String,
    /// Empty for a `package_upload` draft, which belongs to the workspace.
    project_id: Option<Uuid>,
    target: Value,
    author: Author,
    status: String,
    valid: bool,
    created_at: Timestamp,
}

#[derive(Debug, Serialize)]
struct Review {
    by: Option<Uuid>,
    at: Option<Timestamp>,
    comment: Option<String>,
}

/// `Draft` of contract C3 section 7.
#[derive(Debug, Serialize)]
pub struct Draft {
    #[serde(flatten)]
    summary: DraftSummary,
    spec: Value,
    note: Option<String>,
    validation: Value,
    capability_diff: Value,
    /// Recomputed on every read: what the pages of published sites would show.
    public: Vec<Value>,
    review_digest: String,
    review: Option<Review>,
    result: Option<Value>,
}

fn summary(row: &DraftRow) -> DraftSummary {
    DraftSummary {
        id: row.id,
        kind: row.kind.clone(),
        project_id: row.project_id,
        target: row.target.clone(),
        author: Author {
            kind: row.author_kind.clone(),
            id: row.author_id,
            name: row.author_name.clone(),
        },
        status: row.status.clone(),
        valid: row.validation.get("valid").and_then(Value::as_bool).unwrap_or(false),
        created_at: row.created_at,
    }
}

async fn body(db: &impl ConnectionTrait, row: &DraftRow) -> Result<Draft, ApiError> {
    let public = public_exposure(db, row.workspace_id, row.project_id, &row.kind, &row.target, &row.spec).await?;
    let digest = review_digest(&row.spec, &row.validation, &row.capability_diff, &public)?;
    Ok(Draft {
        summary: summary(row),
        spec: row.spec.clone(),
        note: row.note.clone(),
        validation: row.validation.clone(),
        capability_diff: row.capability_diff.clone(),
        public,
        review_digest: digest,
        review: row.reviewed_at.map(|at| Review {
            by: row.reviewed_by,
            at: Some(at),
            comment: row.review_comment.clone(),
        }),
        result: row.result.clone(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDraftRequest {
    kind: String,
    project_id: Uuid,
    target: Value,
    spec: Value,
    note: Option<String>,
}

fn parse_target<T: for<'de> Deserialize<'de>>(target: Value) -> Result<T, ApiError> {
    serde_json::from_value(target).map_err(|_| {
        ApiError::bad_request_coded(
            "target_invalid",
            "target does not match the draft kind (see the contract for its fields)",
        )
    })
}

/// A judged draft before it is stored.
struct Candidate {
    target: Value,
    identity: String,
    prepared: Prepared,
}

async fn form_candidate(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    project: &Scope,
    raw_target: Value,
    spec: Value,
) -> Result<Candidate, ApiError> {
    let input: FormTarget = parse_target(raw_target)?;
    if input.base_schema_version.is_some() {
        return Err(ApiError::bad_request_coded(
            "target_invalid",
            "base_schema_version is recorded by the server",
        ));
    }
    let target = if let Some(form_id) = input.form_id {
        if input.key.is_some() || input.name.is_some() {
            return Err(ApiError::bad_request_coded(
                "target_invalid",
                "key and name are for a new form; an existing form is named by form_id only",
            ));
        }
        let form = find_form(state, form_id).await?;
        if form.project_id != project.id {
            return Err(ApiError::not_found("form not found"));
        }
        require_form_action(state, claims, bot, &form, "form.view").await?;
        if form.archived_at.is_some() {
            return Err(ApiError::conflict_coded("form_archived", "the form is archived", None));
        }
        FormTarget {
            form_id: Some(form_id),
            base_schema_version: Some(form.schema_version),
            key: None,
            name: None,
        }
    } else {
        let (key, name) = prepare::new_form_identity(&input)?;
        prepare::ensure_key_free(&state.db, project.id, &key).await?;
        FormTarget {
            form_id: None,
            base_schema_version: None,
            key: Some(key),
            name: Some(name),
        }
    };
    Ok(Candidate {
        identity: target.identity(),
        prepared: prepare::form_schema(project.workspace_id, project.id, spec),
        target: serde_json::to_value(&target).map_err(|_| ApiError::internal())?,
    })
}

async fn layout_candidate(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    who: &Principal,
    project: &Scope,
    raw_target: Value,
    spec: Value,
) -> Result<Candidate, ApiError> {
    let target: LayoutTarget = parse_target(raw_target)?;
    let kind = ScopeKind::parse(&target.scope_kind)?;
    let scope = resolve_scope(&state.db, kind, target.scope_id).await?;
    if scope.workspace_id != project.workspace_id || scope.project_id.is_some_and(|id| id != project.id) {
        return Err(ApiError::not_found("scope not found"));
    }
    require_read(state, who, &scope).await?;
    if kind == ScopeKind::Form {
        let form = find_form(state, scope.id).await?;
        require_form_action(state, claims, bot, &form, "form.view").await?;
    }
    let slot = draft_slot(&target.slot_id, kind)?;
    Ok(Candidate {
        identity: target.identity(),
        prepared: prepare::layout(&slot, spec),
        target: serde_json::to_value(&target).map_err(|_| ApiError::internal())?,
    })
}

/// `POST /api/v1/drafts`
pub async fn create_draft(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<CreateDraftRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let bot_context = bot.as_ref().map(|Extension(context)| context.clone());
    let who = Principal::new(claims.clone(), bot);
    let project = resolve_project(&state.db, req.project_id).await?;
    member(&state, &who, project.workspace_id).await?;
    if serde_json::to_vec(&req.spec).map_or(true, |bytes| bytes.len() > MAX_SPEC_BYTES) {
        return Err(ApiError::bad_request_coded(
            "draft_too_large",
            "the spec is larger than a draft may be",
        ));
    }
    if req
        .note
        .as_deref()
        .is_some_and(|note| note.chars().count() > MAX_COMMENT_CHARS)
    {
        return Err(ApiError::bad_request_coded(
            "note_too_long",
            "the note is longer than 2000 characters",
        ));
    }
    let (author_kind, author_id) = match who.bot_id() {
        Some(bot_id) => ("bot", bot_id),
        None => ("user", who.user_id()?.ok_or_else(ApiError::internal)?),
    };
    // A throttled author costs no validation: the locked recount in `store_draft` stays the authority.
    if store::count_last_minute(&state.db, author_kind, author_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    let candidate = match req.kind.as_str() {
        KIND_FORM_SCHEMA => {
            form_candidate(&state, &claims, bot_context.as_ref(), &project, req.target, req.spec).await?
        }
        KIND_LAYOUT => {
            layout_candidate(
                &state,
                &claims,
                bot_context.as_ref(),
                &who,
                &project,
                req.target,
                req.spec,
            )
            .await?
        }
        KIND_SITE_PAGE => {
            return Err(ApiError::bad_request_coded(
                "draft_kind_unsupported",
                "site_page drafts are created by the site page and title routes",
            ));
        }
        KIND_PACKAGE_UPLOAD => {
            return Err(ApiError::bad_request_coded(
                "draft_kind_unsupported",
                "package_upload drafts are created by the package upload submission route",
            ));
        }
        "package_install" => {
            return Err(ApiError::bad_request_coded(
                "draft_kind_unsupported",
                "this draft kind is not available yet",
            ));
        }
        _ => {
            return Err(ApiError::bad_request_coded(
                "draft_kind_invalid",
                "kind must be one of: form_schema, layout",
            ));
        }
    };
    let row = store_draft(
        &state,
        &project,
        &req.kind,
        (author_kind, author_id),
        candidate,
        req.note.as_deref(),
    )
    .await?;
    Ok(ApiResponse::success(body(&state.db, &row).await?))
}

/// Stores a judged draft under the author's quotas, superseding the author's older pending
/// drafts for the same target. The public-exposure list is computed here, so the stored digest
/// covers it.
async fn store_draft(
    state: &AppState,
    project: &Scope,
    kind: &str,
    (author_kind, author_id): (&str, Uuid),
    mut candidate: Candidate,
    note: Option<&str>,
) -> Result<DraftRow, ApiError> {
    candidate.prepared.public = public_exposure(
        &state.db,
        project.workspace_id,
        Some(project.id),
        kind,
        &candidate.target,
        &candidate.prepared.spec,
    )
    .await?;
    let digest = candidate.prepared.digest()?;

    let tx = state.db.begin().await?;
    store::lock_author(&tx, author_id).await?;
    store::supersede(&tx, Some(project.id), kind, author_kind, author_id, &candidate.identity).await?;
    if store::count_last_minute(&tx, author_kind, author_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    if store::count_pending(&tx, project.id, author_kind, author_id).await? >= PENDING_PER_AUTHOR {
        return Err(rate_limited("too many pending drafts in this project"));
    }
    let row = store::insert(
        &tx,
        &NewDraft {
            workspace_id: project.workspace_id,
            project_id: Some(project.id),
            kind,
            target: &candidate.target,
            spec: &candidate.prepared.spec,
            author_kind,
            author_id,
            note,
            validation: &candidate.prepared.validation,
            capability_diff: &candidate.prepared.capability_diff,
            review_digest: &digest,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// Stores a bot's form schema save as a `form_schema` draft (LEAD BOT-SCHEMA: the save would
/// change what visitors of a published site get) and returns its id. The form route has already
/// checked the bot's `form.design` on the form; the draft records the form's current schema
/// version as its base.
pub(crate) async fn create_form_schema_draft(
    state: &AppState,
    bot_id: Uuid,
    form: (Uuid, Uuid, Uuid, i32),
    schema: Value,
) -> Result<Uuid, ApiError> {
    let (workspace_id, project_id, form_id, schema_version) = form;
    if serde_json::to_vec(&schema).map_or(true, |bytes| bytes.len() > MAX_SPEC_BYTES) {
        return Err(ApiError::bad_request_coded(
            "draft_too_large",
            "the spec is larger than a draft may be",
        ));
    }
    if store::count_last_minute(&state.db, "bot", bot_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    let project = Scope {
        kind: ScopeKind::Project,
        id: project_id,
        workspace_id,
        project_id: Some(project_id),
    };
    let target = FormTarget {
        form_id: Some(form_id),
        base_schema_version: Some(schema_version),
        key: None,
        name: None,
    };
    let candidate = Candidate {
        identity: target.identity(),
        prepared: prepare::form_schema(workspace_id, project_id, schema),
        target: serde_json::to_value(&target).map_err(|_| ApiError::internal())?,
    };
    let row = store_draft(state, &project, KIND_FORM_SCHEMA, ("bot", bot_id), candidate, None).await?;
    Ok(row.id)
}

/// Stores a bot's write to a published site as a `site_page` draft (contract C3 sections 3 and
/// 6.6, LEAD C3-6) and returns its id. The site route has already checked the bot's write level
/// in the site's project; the draft is judged like any other and an invalid one is stored with
/// its report.
pub(crate) async fn create_site_page_draft(
    state: &AppState,
    (author_kind, author_id): (&'static str, Uuid),
    workspace_id: Uuid,
    project_id: Uuid,
    target: &PageTarget,
    spec: Value,
) -> Result<Uuid, ApiError> {
    if serde_json::to_vec(&spec).map_or(true, |bytes| bytes.len() > MAX_SPEC_BYTES) {
        return Err(ApiError::bad_request_coded(
            "draft_too_large",
            "the spec is larger than a draft may be",
        ));
    }
    if store::count_last_minute(&state.db, author_kind, author_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    let project = Scope {
        kind: ScopeKind::Project,
        id: project_id,
        workspace_id,
        project_id: Some(project_id),
    };
    let prepared = prepare::site_page(&state.db, workspace_id, project_id, target, &spec).await?;
    let candidate = Candidate {
        identity: target.identity(&prepared.spec),
        target: serde_json::to_value(target).map_err(|_| ApiError::internal())?,
        prepared,
    };
    let row = store_draft(
        state,
        &project,
        KIND_SITE_PAGE,
        (author_kind, author_id),
        candidate,
        None,
    )
    .await?;
    Ok(row.id)
}

/// The refusals of a package submission that need no body: expired uploads are cleared first, then
/// the bot's drafts per minute and the workspace's pending uploads are checked, so a refused
/// submission costs no buffering and no verification.
pub(crate) async fn precheck_package_upload(
    state: &AppState,
    workspace_id: Uuid,
    bot_id: Uuid,
) -> Result<(), ApiError> {
    store::expire_uploads(&state.db, workspace_id).await?;
    if store::count_last_minute(&state.db, "bot", bot_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    if store::count_pending_uploads(&state.db, workspace_id).await? >= PENDING_UPLOADS_PER_WORKSPACE {
        return Err(upload_quota());
    }
    Ok(())
}

fn upload_quota() -> ApiError {
    ApiError::conflict_coded(
        "package_upload_quota_exceeded",
        "this workspace or this bot has the most pending package uploads it may; a person must approve or reject some first",
        Some(json!({ "limit": PENDING_UPLOADS_PER_WORKSPACE, "limit_per_bot": PENDING_UPLOADS_PER_BOT })),
    )
}

/// Keeps a verified package upload of a bot as a `package_upload` draft (LEAD-DECISIONS
/// M11-UPLOAD): the bytes are stored with the draft, never as a package. A newer upload of the
/// same bot for the same package version replaces its older pending one; the workspace holds a
/// small number of pending uploads in all, so bots cannot fill the package quota or the database.
pub(crate) async fn create_package_upload_draft(
    state: &AppState,
    workspace_id: Uuid,
    bot_id: Uuid,
    upload: PreparedUpload,
) -> Result<Draft, ApiError> {
    if store::count_last_minute(&state.db, "bot", bot_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    let info = upload.info();
    let prepared = prepare::package_upload(&info);
    let digest = prepared.digest()?;
    let target = PackageUploadTarget::from_info(&info);
    let target_json = serde_json::to_value(&target).map_err(|_| ApiError::internal())?;

    let tx = state.db.begin().await?;
    store::lock_author(&tx, bot_id).await?;
    store::lock_author(&tx, workspace_id).await?;
    store::supersede(&tx, None, KIND_PACKAGE_UPLOAD, "bot", bot_id, &target.identity()).await?;
    if store::count_last_minute(&tx, "bot", bot_id).await? >= CREATIONS_PER_MINUTE {
        return Err(rate_limited("too many drafts in the last minute"));
    }
    if store::count_pending_uploads(&tx, workspace_id).await? >= PENDING_UPLOADS_PER_WORKSPACE
        || store::count_pending_uploads_of_bot(&tx, workspace_id, bot_id).await? >= PENDING_UPLOADS_PER_BOT
    {
        return Err(upload_quota());
    }
    let row = store::insert(
        &tx,
        &NewDraft {
            workspace_id,
            project_id: None,
            kind: KIND_PACKAGE_UPLOAD,
            target: &target_json,
            spec: &prepared.spec,
            author_kind: "bot",
            author_id: bot_id,
            note: None,
            validation: &prepared.validation,
            capability_diff: &prepared.capability_diff,
            review_digest: &digest,
        },
    )
    .await?;
    store::put_upload_blob(&tx, row.id, workspace_id, &info.blob_digest, upload.bytes()).await?;
    tx.commit().await?;
    body(&state.db, &row).await
}

#[derive(Debug, Deserialize)]
pub struct ListDraftsQuery {
    project_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    status: Option<String>,
    kind: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// `GET /api/v1/drafts?project_id=&status=&kind=`: the drafts of the projects the caller can read.
/// A person names a project or a workspace; a bot's workspace is its own.
pub async fn list_drafts(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Query(query): Query<ListDraftsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let workspace_id = match (query.project_id, query.workspace_id) {
        (Some(project_id), _) => {
            let project = resolve_project(&state.db, project_id).await?;
            require_read(&state, &who, &project).await?;
            project.workspace_id
        }
        (None, Some(workspace_id)) => workspace_id,
        (None, None) => {
            return Err(ApiError::bad_request_coded(
                "scope_required",
                "name a project_id or a workspace_id",
            ));
        }
    };
    let role = member(&state, &who, workspace_id).await?;
    store::expire_uploads(&state.db, workspace_id).await?;
    let readable = caller_projects(&state.db, who.bot_id()).await?.ids();
    let paging = PageQuery {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve(50, 200);
    let (rows, total) = store::list(
        &state.db,
        &ListFilter {
            workspace_id,
            project_id: query.project_id,
            readable_projects: readable,
            status: query.status.as_deref(),
            kind: query.kind.as_deref(),
            can_bypass_form_view: role_is_form_admin(&role),
            role_subject: normalize_permission_subject_id(&role)?,
            viewer_bot: who.bot_id(),
            review_uploads: who.bot_id().is_none() && role_is_form_admin(&role),
        },
        paging.per_page,
        paging.offset(),
    )
    .await?;
    let items = rows.iter().map(summary).collect();
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}

/// `GET /api/v1/drafts/{id}`
pub async fn get_draft(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let bot_context = bot.as_ref().map(|Extension(context)| context.clone());
    let who = Principal::new(claims.clone(), bot);
    let row = store::load(&state.db, id, false).await?.ok_or_else(draft_not_found)?;
    // A package upload belongs to the workspace: the people who review it and the bot that
    // submitted it read it, nobody else: not another bot, whatever its permissions, and not a
    // project-scoped bot.
    if row.kind == KIND_PACKAGE_UPLOAD {
        let role = member(&state, &who, row.workspace_id)
            .await
            .map_err(reads_as_not_found)?;
        let own = who
            .bot_id()
            .is_some_and(|bot_id| row.author_kind == "bot" && row.author_id == bot_id);
        if !(own || (who.bot_id().is_none() && role_is_form_admin(&role))) {
            return Err(draft_not_found());
        }
        return Ok(ApiResponse::success(body(&state.db, &row).await?));
    }
    let project_id = row.project()?;
    let project = Scope {
        kind: ScopeKind::Project,
        id: project_id,
        workspace_id: row.workspace_id,
        project_id: Some(project_id),
    };
    require_read(&state, &who, &project).await.map_err(reads_as_not_found)?;
    // The proposed schema of a form, and a layout placed on a form, are as private as the form:
    // whoever may not view it gets nothing, and learns nothing about whether the draft exists.
    if let Some(form_id) = guarded_form(&row)? {
        let form = find_form(&state, form_id).await.map_err(reads_as_not_found)?;
        require_form_action(&state, &claims, bot_context.as_ref(), &form, "form.view")
            .await
            .map_err(reads_as_not_found)?;
    }
    Ok(ApiResponse::success(body(&state.db, &row).await?))
}

/// The person who reviews, with the role that lets them: an owner or an administrator of the
/// draft's workspace. A bot never gets here (the route takes user sessions only); a connected
/// application works as a plain member, which fails the role check.
async fn reviewer(state: &AppState, claims: &JwtClaims, row: &DraftRow) -> Result<(Uuid, String), ApiError> {
    let who = Principal::new(claims.clone(), None);
    let user_id = who
        .user_id()?
        .ok_or_else(|| ApiError::forbidden("drafts are reviewed by people"))?;
    let role = member(state, &who, row.workspace_id)
        .await
        .map_err(reads_as_not_found)?;
    if !role_is_form_admin(&role) {
        return Err(ApiError::forbidden("only workspace owners and admins review drafts"));
    }
    Ok((user_id, role))
}

fn ensure_pending(row: &DraftRow) -> Result<(), ApiError> {
    if row.status == "pending" {
        return Ok(());
    }
    Err(ApiError::conflict_coded(
        "draft_not_pending",
        "the draft is no longer pending",
        Some(json!({ "status": row.status })),
    ))
}

fn check_comment(comment: Option<&str>) -> Result<(), ApiError> {
    if comment.is_some_and(|text| text.chars().count() > MAX_COMMENT_CHARS) {
        return Err(ApiError::bad_request_coded(
            "comment_too_long",
            "the comment is longer than 2000 characters",
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveRequest {
    review_digest: String,
    comment: Option<String>,
    /// For package installs; none of the kinds here takes grants.
    grants: Option<Vec<Value>>,
}

/// `POST /api/v1/drafts/{id}/approve`: judges the draft again against the current state.
///
/// When the digest the reviewer saw is no longer the digest of that state, stores the new report
/// and diff and refuses `draft_stale`. Otherwise saves through the ordinary path and closes the draft.
pub async fn approve_draft(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
    Json(req): Json<ApproveRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_comment(req.comment.as_deref())?;
    if req.grants.as_ref().is_some_and(|grants| !grants.is_empty()) {
        return Err(ApiError::bad_request_coded(
            "grants_not_applicable",
            "this draft kind takes no grants",
        ));
    }
    let tx = state.db.begin().await?;
    let row = store::load(&tx, id, true).await?.ok_or_else(draft_not_found)?;
    let (approver, role) = reviewer(&state, &claims, &row).await?;
    ensure_pending(&row)?;
    let (prepared, plan) = prepare::recompute(&tx, &row).await?;
    let digest = prepared.digest()?;
    if !prepared.valid() {
        store::refresh_review(&tx, id, &prepared.validation, &prepared.capability_diff, &digest).await?;
        tx.commit().await?;
        return Err(ApiError::Coded {
            status: 400,
            code: "draft_invalid",
            message: "the draft does not validate".to_owned(),
            details: Some(json!({ "validation": prepared.validation })),
        });
    }
    if digest != req.review_digest {
        store::refresh_review(&tx, id, &prepared.validation, &prepared.capability_diff, &digest).await?;
        tx.commit().await?;
        return Err(ApiError::conflict_coded(
            "draft_stale",
            "the draft changed since it was reviewed; review the new state",
            Some(json!({ "review_digest": digest })),
        ));
    }
    let result = apply::save(&state, &claims, &tx, &row, plan, &prepared, approver, &role).await?;
    store::close(&tx, id, "approved", approver, req.comment.as_deref(), Some(&result)).await?;
    let closed = store::load(&tx, id, false).await?.ok_or_else(ApiError::internal)?;
    tx.commit().await?;
    Ok(ApiResponse::success(body(&state.db, &closed).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectRequest {
    comment: String,
}

/// `POST /api/v1/drafts/{id}/reject`
pub async fn reject_draft(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
    Json(req): Json<RejectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_comment(Some(&req.comment))?;
    if req.comment.trim().is_empty() {
        return Err(ApiError::bad_request_coded(
            "comment_required",
            "say why the draft is rejected",
        ));
    }
    let tx = state.db.begin().await?;
    let row = store::load(&tx, id, true).await?.ok_or_else(draft_not_found)?;
    let (reviewer_id, _) = reviewer(&state, &claims, &row).await?;
    ensure_pending(&row)?;
    store::close(&tx, id, "rejected", reviewer_id, Some(&req.comment), None).await?;
    let closed = store::load(&tx, id, false).await?.ok_or_else(ApiError::internal)?;
    tx.commit().await?;
    Ok(ApiResponse::success(body(&state.db, &closed).await?))
}
