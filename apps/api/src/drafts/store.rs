//! SQL access to `artefact_drafts`.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

const COLUMNS: &str = "d.id, d.workspace_id, d.project_id, d.kind, d.target, d.spec, d.author_kind, d.author_id, \
     CASE d.author_kind \
       WHEN 'bot' THEN (SELECT b.name::text FROM workspace_bots b WHERE b.id = d.author_id) \
       ELSE (SELECT u.name::text FROM users u WHERE u.id = d.author_id) END AS author_name, \
     d.note, d.validation, d.capability_diff, d.status, d.reviewed_by, d.reviewed_at, \
     d.review_comment, d.result, d.created_at";

#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct DraftRow {
    pub(super) id: Uuid,
    pub(super) workspace_id: Uuid,
    pub(super) project_id: Uuid,
    pub(super) kind: String,
    pub(super) target: Value,
    pub(super) spec: Value,
    pub(super) author_kind: String,
    pub(super) author_id: Uuid,
    pub(super) author_name: Option<String>,
    pub(super) note: Option<String>,
    pub(super) validation: Value,
    pub(super) capability_diff: Value,
    pub(super) status: String,
    pub(super) reviewed_by: Option<Uuid>,
    pub(super) reviewed_at: Option<Timestamp>,
    pub(super) review_comment: Option<String>,
    pub(super) result: Option<Value>,
    pub(super) created_at: Timestamp,
}

/// The values of a new draft.
pub(super) struct NewDraft<'a> {
    pub(super) workspace_id: Uuid,
    pub(super) project_id: Uuid,
    pub(super) kind: &'a str,
    pub(super) target: &'a Value,
    pub(super) spec: &'a Value,
    pub(super) author_kind: &'a str,
    pub(super) author_id: Uuid,
    pub(super) note: Option<&'a str>,
    pub(super) validation: &'a Value,
    pub(super) capability_diff: &'a Value,
    pub(super) review_digest: &'a str,
}

#[derive(FromQueryResult)]
struct Count {
    total: i64,
}

#[derive(FromQueryResult)]
struct Id {
    id: Uuid,
}

/// Serialises the creations of one author for the rest of the transaction, so the quota checks
/// that follow cannot be raced past.
pub(super) async fn lock_author(db: &impl ConnectionTrait, author_id: Uuid) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        vec![format!("artefact_draft_author:{author_id}").into()],
    ))
    .await?;
    Ok(())
}

pub(super) async fn count_pending(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    author_kind: &str,
    author_id: Uuid,
) -> Result<i64, ApiError> {
    Ok(Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM artefact_drafts \
         WHERE project_id = $1 AND author_kind = $2 AND author_id = $3 AND status = 'pending'",
        vec![project_id.into(), author_kind.to_owned().into(), author_id.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.total))
}

/// Drafts of one author created in the last minute, across projects.
pub(super) async fn count_last_minute(
    db: &impl ConnectionTrait,
    author_kind: &str,
    author_id: Uuid,
) -> Result<i64, ApiError> {
    Ok(Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM artefact_drafts \
         WHERE author_kind = $1 AND author_id = $2 AND created_at > now() - interval '1 minute'",
        vec![author_kind.to_owned().into(), author_id.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.total))
}

/// Marks the older pending drafts of the same author for the same target `superseded`.
pub(super) async fn supersede(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    kind: &str,
    author_kind: &str,
    author_id: Uuid,
    identity: &str,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE artefact_drafts SET status = 'superseded' \
         WHERE project_id = $1 AND kind = $2 AND author_kind = $3 AND author_id = $4 \
           AND status = 'pending' AND CASE kind \
              WHEN 'form_schema' THEN COALESCE(target->>'form_id', 'new:' || (target->>'key')) \
              WHEN 'layout' THEN (target->>'slot_id') || '/' || (target->>'scope_kind') || '/' || (target->>'scope_id') \
              ELSE target::text END = $5",
        vec![
            project_id.into(),
            kind.to_owned().into(),
            author_kind.to_owned().into(),
            author_id.into(),
            identity.to_owned().into(),
        ],
    ))
    .await?;
    Ok(())
}

pub(super) async fn insert(db: &impl ConnectionTrait, draft: &NewDraft<'_>) -> Result<DraftRow, ApiError> {
    let created = Id::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO artefact_drafts \
           (workspace_id, project_id, kind, target, spec, author_kind, author_id, note, validation, \
            capability_diff, review_digest) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
        vec![
            draft.workspace_id.into(),
            draft.project_id.into(),
            draft.kind.to_owned().into(),
            draft.target.clone().into(),
            draft.spec.clone().into(),
            draft.author_kind.to_owned().into(),
            draft.author_id.into(),
            draft.note.map(str::to_owned).into(),
            draft.validation.clone().into(),
            draft.capability_diff.clone().into(),
            draft.review_digest.to_owned().into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)?;
    load(db, created.id, false).await?.ok_or_else(ApiError::internal)
}

/// Loads one draft; `lock` takes the row lock a review needs.
pub(super) async fn load(db: &impl ConnectionTrait, id: Uuid, lock: bool) -> Result<Option<DraftRow>, ApiError> {
    let suffix = if lock { " FOR UPDATE OF d" } else { "" };
    Ok(DraftRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {COLUMNS} FROM artefact_drafts d WHERE d.id = $1{suffix}"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

/// What a list asks for.
pub(super) struct ListFilter<'a> {
    pub(super) workspace_id: Uuid,
    pub(super) project_id: Option<Uuid>,
    /// `None`: no limit beyond the workspace.
    pub(super) readable_projects: Option<Vec<Uuid>>,
    pub(super) status: Option<&'a str>,
    pub(super) kind: Option<&'a str>,
}

pub(super) async fn list(
    db: &impl ConnectionTrait,
    filter: &ListFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<DraftRow>, i64), ApiError> {
    const WHERE: &str = "d.workspace_id = $1 \
         AND ($2::uuid IS NULL OR d.project_id = $2) \
         AND ($3::uuid[] IS NULL OR d.project_id = ANY($3)) \
         AND ($4::text IS NULL OR d.status = $4) \
         AND ($5::text IS NULL OR d.kind = $5)";
    let values = || -> Vec<sea_orm::Value> {
        vec![
            filter.workspace_id.into(),
            filter.project_id.into(),
            filter.readable_projects.clone().into(),
            filter.status.map(str::to_owned).into(),
            filter.kind.map(str::to_owned).into(),
        ]
    };
    let total = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT COUNT(*)::bigint AS total FROM artefact_drafts d WHERE {WHERE}"),
        values(),
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.total);
    let mut page_values = values();
    page_values.push(limit.into());
    page_values.push(offset.into());
    let rows = DraftRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {COLUMNS} FROM artefact_drafts d WHERE {WHERE} \
             ORDER BY d.created_at DESC, d.id LIMIT $6 OFFSET $7"
        ),
        page_values,
    ))
    .all(db)
    .await?;
    Ok((rows, total))
}

/// Stores the report, diff and digest a review recomputed.
pub(super) async fn refresh_review(
    db: &impl ConnectionTrait,
    id: Uuid,
    validation: &Value,
    capability_diff: &Value,
    review_digest: &str,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE artefact_drafts SET validation = $2, capability_diff = $3, review_digest = $4 WHERE id = $1",
        vec![
            id.into(),
            validation.clone().into(),
            capability_diff.clone().into(),
            review_digest.to_owned().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Closes a pending draft as `approved` or `rejected`.
pub(super) async fn close(
    db: &impl ConnectionTrait,
    id: Uuid,
    status: &str,
    reviewer: Uuid,
    comment: Option<&str>,
    result: Option<&Value>,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE artefact_drafts \
         SET status = $2, reviewed_by = $3, reviewed_at = now(), review_comment = $4, result = $5 \
         WHERE id = $1 AND status = 'pending'",
        vec![
            id.into(),
            status.to_owned().into(),
            reviewer.into(),
            comment.map(str::to_owned).into(),
            result.cloned().into(),
        ],
    ))
    .await?;
    Ok(())
}
