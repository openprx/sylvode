//! The target of an operation, resolved for the catalogue checker (contract C1 §2.3).

use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use crate::catalogue::{Entry, Find, Principal, Resource, Scope};
use crate::middleware::scoped_bot::{Target, resolve};

/// The UUID path parameter the entry's `Find` resolves, which the host holds to the canonical form.
///
/// The identifier of an issue (`PREFIX-N`) and the ids of proposals, decisions and
/// proposal comments are not UUIDs; they pass the plain-value check like every other parameter.
#[must_use]
pub const fn primary_param(entry: &Entry) -> Option<&'static str> {
    match entry.scope {
        Scope::Project(
            Find::Project(name)
            | Find::Issue(name)
            | Find::Comment(name)
            | Find::Table(_, name)
            | Find::FlowObject(name)
            | Find::FlowScope(name)
            | Find::FlowVia(_, _, _, name),
        ) => Some(name),
        _ => None,
    }
}

/// Whether the host must read the request body and hand it to [`resolve_resource`]: the target is
/// named there, or the route works on a workspace the body may name ([`workspace_pinned`]).
#[must_use]
pub const fn resolves_from_body(entry: &Entry) -> bool {
    matches!(entry.scope, Scope::Project(Find::Body | Find::Conversion)) || workspace_wide(entry)
}

/// Whether the entry names no project (`Global`, `Own`, `Filtered`), so the workspace it works in
/// is whatever the request says: [`workspace_pinned`] holds that to the installation's.
#[must_use]
pub const fn workspace_wide(entry: &Entry) -> bool {
    matches!(entry.scope, Scope::Global | Scope::Own | Scope::Filtered)
}

/// Whether every workspace the request names is the installation's.
///
/// The names are the `workspace_id` path parameter, query key and top-level body field. A value
/// that is not a UUID does not match, so a malformed id fails closed.
#[must_use]
pub fn workspace_pinned(
    workspace_id: Uuid,
    params: &[(String, String)],
    query: Option<&str>,
    body: Option<&serde_json::Value>,
) -> bool {
    let matches = |value: &str| Uuid::parse_str(value).is_ok_and(|id| id == workspace_id);
    let in_path = params
        .iter()
        .filter(|(key, _)| key == "workspace_id")
        .all(|(_, value)| matches(value));
    let in_query = query.is_none_or(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .filter(|(key, _)| key == "workspace_id")
            .all(|(_, value)| matches(&value))
    });
    let in_body = body
        .and_then(|body| body.get("workspace_id"))
        .is_none_or(|value| value.is_null() || value.as_str().is_some_and(matches));
    in_path && in_query && in_body
}

/// The parameter in canonical UUID form (lowercase, hyphenated), or `None`.
#[must_use]
pub fn canonical_uuid(value: &str) -> Option<Uuid> {
    let parsed = Uuid::parse_str(value).ok()?;
    (parsed.hyphenated().to_string() == value).then_some(parsed)
}

#[derive(FromQueryResult)]
struct FormRow {
    form_id: Uuid,
}

/// The tables whose rows belong to a form (`form_id` column), for the form selector of a grant.
const FORM_CHILD_TABLES: [&str; 6] = [
    "form_views",
    "form_attachments",
    "form_import_jobs",
    "form_export_jobs",
    "form_import_mapping_templates",
    "form_attachment_package_jobs",
];

/// The workspace of the installation a plugin principal acts for.
const fn installation_workspace(principal: &Principal) -> Option<Uuid> {
    match principal {
        Principal::Plugin { installation, .. } | Principal::Installation { installation, .. } => {
            Some(installation.workspace_id)
        }
        _ => None,
    }
}

/// Resolves the target of `entry` for the request a plugin call makes: the path parameters
/// `params`, the raw `query` and the JSON `body`, exactly as the host sends them.
///
/// * A project scope resolves through the same finder a scoped bot is judged by, with the nil bot:
///   a personal-space Flow object, a proposal no project claims and any other "own" target resolve
///   to nothing, and the installation's workspace is the workspace of an issue identifier.
/// * A `Global`, `Own` or `Filtered` scope names no project: it resolves to the installation's
///   workspace alone, and the checker admits it only for a workspace installation with an
///   unrestricted grant.
/// * Every other scope has no plugin target.
///
/// `None` when there is no such target, or the entry has none; callers answer both like a target
/// outside the workspace, so an answer never says whether an id exists.
///
/// # Errors
/// A database failure.
pub async fn resolve_resource(
    db: &DatabaseConnection,
    entry: &Entry,
    principal: &Principal,
    params: &[(String, String)],
    query: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Result<Option<Resource>, crate::error::ApiError> {
    let Some(installation_workspace) = installation_workspace(principal) else {
        return Ok(None);
    };
    let find = match entry.scope {
        Scope::Project(find) => find,
        Scope::Global | Scope::Own | Scope::Filtered => {
            return Ok(Some(Resource {
                workspace_id: installation_workspace,
                project_id: None,
                form_id: None,
            }));
        }
        _ => return Ok(None),
    };
    let target = resolve(db, find, params, query, body, Uuid::nil(), installation_workspace).await?;
    let Target::Project {
        workspace_id,
        project_id,
    } = target
    else {
        return Ok(None);
    };
    let form_id = form_of(db, find, params).await?;
    Ok(Some(Resource {
        workspace_id,
        project_id: Some(project_id),
        form_id,
    }))
}

async fn form_of(
    db: &DatabaseConnection,
    find: Find,
    params: &[(String, String)],
) -> Result<Option<Uuid>, crate::error::ApiError> {
    let Find::Table(table, name) = find else {
        return Ok(None);
    };
    let Some(value) = params.iter().find(|(key, _)| key == name).map(|(_, value)| value) else {
        return Ok(None);
    };
    let sql = match table {
        "project_forms" => return Ok(canonical_uuid(value)),
        "form_records" => "SELECT form_id FROM form_records WHERE id::text = $1".to_string(),
        child if FORM_CHILD_TABLES.contains(&child) => format!("SELECT form_id FROM {child} WHERE id::text = $1"),
        _ => return Ok(None),
    };
    let row = FormRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        vec![value.clone().into()],
    ))
    .one(db)
    .await
    .map_err(crate::error::ApiError::internal_database_failure)?;
    Ok(row.map(|row| row.form_id))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{canonical_uuid, primary_param};
    use crate::catalogue::by_op;

    #[test]
    fn only_canonical_uuids_pass() {
        let id = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        assert!(canonical_uuid(id).is_some());
        assert!(canonical_uuid(&id.to_uppercase()).is_none());
        assert!(canonical_uuid(&id.replace('-', "")).is_none());
        assert!(canonical_uuid("{0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f}").is_none());
        assert!(canonical_uuid("x").is_none());
    }

    #[test]
    fn the_primary_parameter_is_the_one_the_find_names() {
        let entry = by_op("forms.records.update").expect("entry");
        assert_eq!(primary_param(entry), Some("record_id"));
        let entry = by_op("forms.records.create").expect("entry");
        assert_eq!(primary_param(entry), Some("form_id"));
    }
}
