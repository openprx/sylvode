//! Records of a site binding: the read scope of each audience, the list and detail reads with
//! filter, sort and the read-time projection, and the submit/update writers through the C2
//! pipeline (contract C3 sections 6.1, 6.2, 6.6).

use std::collections::BTreeSet;

use axum::http::StatusCode;
use platform::app::AppState;
use platform::timestamp::Timestamp;
use sea_orm::{DbBackend, FromQueryResult, Statement, Value as SqlValue};
use serde::Serialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::http::SiteError;
use super::projection::{
    Audience, EffectiveRead, Projection, RECORD_COLUMNS, SiteField, effective_read, effective_write, site_fields,
};
use super::session::{Caller, Operator};
use super::store::{self, BindingRow};
use crate::forms::permissions::{form_action_allowed, form_record_scope};
use crate::forms::read_policy::render_record_title;
use crate::forms::reader_context::ReaderContext;
use crate::forms::rules::write_plan;
use crate::routes::form::{FormResponse, find_form};

/// Largest page of a record list.
pub(crate) const MAX_PER_PAGE: u64 = 100;

/// One binding as a request sees it: the row, its projection, the bound form and its fields.
pub(crate) struct Binding {
    pub(crate) row: BindingRow,
    pub(crate) projection: Projection,
    pub(crate) form: FormResponse,
    pub(crate) fields: Vec<SiteField>,
}

impl Binding {
    /// The binding `key` of the caller's site; a missing binding, or a form that left the site's
    /// project or was archived, is `404`.
    pub(crate) async fn load(state: &AppState, caller: &Caller, key: &str) -> Result<Self, SiteError> {
        let row = store::load_binding(&state.db, caller.site.id, key)
            .await?
            .ok_or_else(SiteError::not_found)?;
        let form = find_form(state, row.form_id)
            .await
            .map_err(|_| SiteError::not_found())?;
        if form.project_id != caller.site.project_id || form.archived_at.is_some() {
            return Err(SiteError::not_found());
        }
        let fields = site_fields(&form.schema)?;
        Ok(Self {
            projection: Projection::stored(&row.projection),
            row,
            form,
            fields,
        })
    }

    fn log_dropped(&self, read: &EffectiveRead) {
        if read.dropped > 0 {
            tracing::warn!(
                metric = "site_projection_dropped_total",
                site = %self.row.site_id,
                binding = %self.row.binding_key,
                count = read.dropped,
                "site projection fields dropped at read"
            );
        }
    }
}

/// How the caller may read a binding's records.
pub(crate) struct ReadAccess {
    pub(crate) read: EffectiveRead,
    scope: Scope,
}

enum Scope {
    /// Records the visitor submitted at this site; no visitor yet means no records.
    Visitor(Option<Uuid>),
    /// Every record, or only the operator's own under an "owned" record scope.
    Operator { only_created_by: Option<Uuid> },
}

/// The operator's own read limits on the form: `form.view`, then the fields their policy excludes.
async fn operator_excluded(
    state: &AppState,
    binding: &Binding,
    operator: &Operator,
) -> Result<BTreeSet<String>, SiteError> {
    if !form_action_allowed(state, binding.form.id, &operator.workspace_role, "form.view").await? {
        return Err(SiteError::not_found());
    }
    let policy = ReaderContext::new(&operator.workspace_role)
        .policy(&state.db, (&binding.form).into())
        .await?;
    Ok(policy.excluded_fields().clone())
}

/// The read access of the caller; `None` when the audience has no read projection.
pub(crate) async fn read_access(
    state: &AppState,
    caller: &Caller,
    binding: &Binding,
) -> Result<Option<ReadAccess>, SiteError> {
    let (audience, excluded) = match &caller.operator {
        Some(operator) => (Audience::Operator, operator_excluded(state, binding, operator).await?),
        None => (Audience::Visitor, BTreeSet::new()),
    };
    let Some(read) = binding.projection.read(audience) else {
        return Ok(None);
    };
    let effective = effective_read(read, &binding.fields, &binding.form.title_template, &excluded);
    binding.log_dropped(&effective);
    let scope = match &caller.operator {
        Some(operator) => {
            let owned = form_record_scope(state, binding.form.id, &operator.workspace_role).await? == "owned";
            Scope::Operator {
                only_created_by: owned.then_some(operator.user_id),
            }
        }
        None => Scope::Visitor(caller.visitor_id),
    };
    Ok(Some(ReadAccess { read: effective, scope }))
}

/// A record as the site shows it.
#[derive(Debug, Serialize)]
pub(crate) struct SiteRecord {
    id: Uuid,
    values: Value,
    title: Option<String>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct RecordRow {
    id: Uuid,
    values: Value,
    created_at: Timestamp,
    updated_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct Count {
    n: i64,
}

/// SQL text and values of a query under construction.
struct Sql {
    text: String,
    values: Vec<SqlValue>,
}

impl Sql {
    fn bind(&mut self, value: impl Into<SqlValue>) -> String {
        self.values.push(value.into());
        format!("${}", self.values.len())
    }
}

/// `form_id`, not archived, inside the caller's scope.
fn scoped_where(sql: &mut Sql, binding: &Binding, site_id: Uuid, scope: &Scope) -> Option<String> {
    let form = sql.bind(binding.form.id);
    let mut clause = format!("form_id = {form} AND archived_at IS NULL");
    match scope {
        Scope::Visitor(None) => return None,
        Scope::Visitor(Some(visitor)) => {
            let site = sql.bind(site_id.to_string());
            let visitor = sql.bind(visitor.to_string());
            clause.push_str(&format!(
                " AND source->>'kind' = 'site' AND source->>'site_id' = {site} AND source->>'site_visitor_id' = {visitor}"
            ));
        }
        Scope::Operator { only_created_by } => {
            if let Some(user) = only_created_by {
                let user = sql.bind(*user);
                clause.push_str(&format!(" AND created_by = {user}"));
            }
        }
    }
    Some(clause)
}

/// A list request: equality filters and one sort key, already parsed.
pub(crate) struct ListRequest {
    pub(crate) page: u64,
    pub(crate) per_page: u64,
    pub(crate) filters: Vec<(String, String)>,
    pub(crate) sort: Option<(String, bool)>,
}

fn not_filterable(key: &str) -> SiteError {
    SiteError::new(
        StatusCode::BAD_REQUEST,
        "field_not_filterable",
        "this field cannot be filtered or sorted here",
    )
    .with_details(json!({ "field": key }))
}

/// One page of the caller's records.
pub(crate) async fn list(
    state: &AppState,
    caller: &Caller,
    binding: &Binding,
    access: &ReadAccess,
    request: &ListRequest,
) -> Result<(Vec<SiteRecord>, i64), SiteError> {
    for (key, _) in &request.filters {
        if !access.read.filter.contains(key) {
            return Err(not_filterable(key));
        }
    }
    if let Some((key, _)) = &request.sort
        && !access.read.sort.contains(key)
    {
        return Err(not_filterable(key));
    }
    let mut sql = Sql {
        text: String::new(),
        values: Vec::new(),
    };
    let Some(mut clause) = scoped_where(&mut sql, binding, caller.site.id, &access.scope) else {
        return Ok((Vec::new(), 0));
    };
    for (key, value) in &request.filters {
        let key = sql.bind(key.clone());
        let value = sql.bind(value.clone());
        clause.push_str(&format!(
            " AND (CASE jsonb_typeof(values->{key}) \
               WHEN 'array' THEN jsonb_exists(values->{key}, {value}) \
               WHEN 'object' THEN values->{key}->>'value' = {value} \
               ELSE values->>{key} = {value} END)"
        ));
    }
    let count_sql = format!("SELECT count(*)::bigint AS n FROM form_records WHERE {clause}");
    let total = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        count_sql,
        sql.values.clone(),
    ))
    .one(&state.db)
    .await?
    .map_or(0, |row| row.n);
    let order = match &request.sort {
        Some((key, descending)) => {
            let direction = if *descending { "DESC" } else { "ASC" };
            let column = RECORD_COLUMNS
                .iter()
                .find(|column| **column == key.as_str() && !binding.fields.iter().any(|field| field.key == *key));
            match column {
                Some(column) => format!("{column} {direction}, id"),
                None => {
                    let key = sql.bind(key.clone());
                    format!(
                        "(CASE jsonb_typeof(values->{key}) WHEN 'object' THEN values->{key}->'value' ELSE values->{key} END) \
                         {direction} NULLS LAST, id"
                    )
                }
            }
        }
        None => "created_at DESC, id".to_string(),
    };
    let limit = sql.bind(request.per_page);
    let offset = sql.bind(request.page.saturating_sub(1).saturating_mul(request.per_page));
    sql.text = format!(
        "SELECT id, values, created_at, updated_at FROM form_records WHERE {clause} \
         ORDER BY {order} LIMIT {limit} OFFSET {offset}"
    );
    let rows = RecordRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql.text,
        sql.values,
    ))
    .all(&state.db)
    .await?;
    let shown = rows
        .into_iter()
        .map(|row| present(binding, &access.read, row))
        .collect();
    Ok((shown, total))
}

/// One record of the caller's scope, or `404`.
pub(crate) async fn get(
    state: &AppState,
    caller: &Caller,
    binding: &Binding,
    access: &ReadAccess,
    id: Uuid,
) -> Result<SiteRecord, SiteError> {
    load_scoped(state, caller, binding, &access.scope, id)
        .await?
        .map(|row| present(binding, &access.read, row))
        .ok_or_else(SiteError::not_found)
}

async fn load_scoped(
    state: &AppState,
    caller: &Caller,
    binding: &Binding,
    scope: &Scope,
    id: Uuid,
) -> Result<Option<RecordRow>, SiteError> {
    let mut sql = Sql {
        text: String::new(),
        values: Vec::new(),
    };
    let Some(clause) = scoped_where(&mut sql, binding, caller.site.id, scope) else {
        return Ok(None);
    };
    let id = sql.bind(id);
    sql.text = format!("SELECT id, values, created_at, updated_at FROM form_records WHERE {clause} AND id = {id}");
    Ok(RecordRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql.text,
        sql.values,
    ))
    .one(&state.db)
    .await?)
}

/// The values the read projection lets through, masked where the schema says so; record columns
/// come from the row; the title only when all its inputs are readable.
fn present(binding: &Binding, read: &EffectiveRead, row: RecordRow) -> SiteRecord {
    let values = project_values(binding, read, &row.values, row.created_at, row.updated_at);
    let title = read.title.then(|| {
        let excluded: BTreeSet<String> = binding
            .fields
            .iter()
            .filter(|field| !read.allows(&field.key))
            .map(|field| field.key.clone())
            .collect();
        render_record_title(&binding.form.title_template, row.id, &row.values, &excluded)
    });
    SiteRecord {
        id: row.id,
        values,
        title,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn project_values(
    binding: &Binding,
    read: &EffectiveRead,
    stored: &Value,
    created_at: Timestamp,
    updated_at: Timestamp,
) -> Value {
    let plan = write_plan(&binding.form.schema).ok();
    let mut out = Map::new();
    for key in &read.fields {
        let is_field = binding.fields.iter().any(|field| field.key == *key);
        if !is_field {
            match key.as_str() {
                "created_at" => out.insert(key.clone(), json!(created_at)),
                "updated_at" => out.insert(key.clone(), json!(updated_at)),
                _ => None,
            };
            continue;
        }
        let Some(value) = stored.get(key) else { continue };
        let value = if read.masked.contains(key) {
            plan.as_ref()
                .and_then(|plan| plan.field(key))
                .and_then(|field_plan| {
                    field_plan
                        .mask
                        .as_ref()
                        .and_then(|mask| field_plan.ty.mask(&field_plan.field, value, mask))
                })
                .unwrap_or(Value::Null)
        } else {
            value.clone()
        };
        out.insert(key.clone(), value);
    }
    Value::Object(out)
}

/// Keys the audience may write, in projection terms; anything else is refused before the pipeline.
pub(crate) fn ensure_writable(values: &Map<String, Value>, allowed: &[String]) -> Result<(), SiteError> {
    let refused: Vec<&String> = values.keys().filter(|key| !allowed.contains(*key)).collect();
    if refused.is_empty() {
        Ok(())
    } else {
        Err(SiteError::new(
            StatusCode::BAD_REQUEST,
            "field_not_allowed",
            "these fields cannot be written here",
        )
        .with_details(json!({ "fields": refused })))
    }
}

/// The submit fields of the visitor projection that still qualify.
pub(crate) fn submit_fields(binding: &Binding) -> Vec<String> {
    binding
        .projection
        .visitor
        .submit
        .as_ref()
        .map(|submit| effective_write(&submit.fields, &binding.fields, &BTreeSet::new()))
        .unwrap_or_default()
}

/// The update fields of the operator projection that still qualify.
pub(crate) fn update_fields(binding: &Binding) -> Vec<String> {
    binding
        .projection
        .operator
        .update
        .as_ref()
        .map(|update| effective_write(&update.fields, &binding.fields, &BTreeSet::new()))
        .unwrap_or_default()
}

/// Only the given keys of a value map.
pub(crate) fn pick(values: &Value, keys: &[String]) -> Value {
    let mut out = Map::new();
    for key in keys {
        if let Some(value) = values.get(key) {
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

/// Field errors of a refused write, limited to the keys the audience may see: an error on any
/// other field is reported as one `constraint_failed` entry without its key.
pub(crate) fn redact_field_errors(error: SiteError, visible: &[String]) -> SiteError {
    if error.code() != crate::forms::rules::VALIDATION_FAILED {
        return error;
    }
    error.map_details(|details| {
        let Some(errors) = details.get("errors").and_then(Value::as_array) else {
            return json!({ "errors": [], "truncated": false });
        };
        let mut kept: Vec<Value> = Vec::new();
        let mut hidden = false;
        for item in errors {
            let field = item.get("field").and_then(Value::as_str).unwrap_or_default();
            if visible.iter().any(|key| key == field) {
                let mut item = item.clone();
                if let Some(object) = item.as_object_mut() {
                    object.remove("field_id");
                    object.insert("path".to_string(), json!(format!("/values/{field}")));
                }
                kept.push(item);
            } else {
                hidden = true;
            }
        }
        if hidden {
            kept.push(json!({ "field": null, "code": "constraint_failed" }));
        }
        json!({ "errors": kept, "truncated": details.get("truncated").cloned().unwrap_or(json!(false)) })
    })
}

/// Loads a record of the operator's scope for an update, with the full stored row.
pub(crate) async fn load_for_update(
    state: &AppState,
    caller: &Caller,
    binding: &Binding,
    access: &ReadAccess,
    id: Uuid,
) -> Result<crate::routes::form::RecordResponse, SiteError> {
    if load_scoped(state, caller, binding, &access.scope, id).await?.is_none() {
        return Err(SiteError::not_found());
    }
    crate::routes::form::find_record(state, id)
        .await
        .map_err(|_| SiteError::not_found())
}
