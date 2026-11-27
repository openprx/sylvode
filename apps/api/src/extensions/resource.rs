//! The target of an operation, resolved for the catalogue checker (contract C1 §2.3).

use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use crate::catalogue::{Entry, Find, Resource, Scope};
use crate::middleware::scoped_bot::{Target, resolve};

/// The path parameter the entry's `Find` resolves, if the entry resolves one.
#[must_use]
pub const fn primary_param(entry: &Entry) -> Option<&'static str> {
    match entry.scope {
        Scope::Project(
            Find::Project(name)
            | Find::Issue(name)
            | Find::IssueIdentifier(name)
            | Find::Comment(name)
            | Find::Table(_, name),
        ) => Some(name),
        _ => None,
    }
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

/// Resolves the target of `entry` for the path parameters `params`.
///
/// `None` when there is no such target, or the entry has no resolvable scope; callers answer both
/// like a target outside the workspace, so an answer never says whether an id exists.
///
/// # Errors
/// A database failure.
pub async fn resolve_resource(
    db: &DatabaseConnection,
    entry: &Entry,
    params: &[(String, String)],
) -> Result<Option<Resource>, crate::error::ApiError> {
    let Scope::Project(find) = entry.scope else {
        return Ok(None);
    };
    // The plugin-reachable `Find`s read only the target row; the bot and workspace arguments
    // matter for `IssueIdentifier` and Flow finds, which are not plugin operations.
    let target = resolve(db, find, params, None, None, Uuid::nil(), Uuid::nil()).await?;
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
    match table {
        "project_forms" => Ok(canonical_uuid(value)),
        "form_records" => {
            let row = FormRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT form_id FROM form_records WHERE id::text = $1",
                vec![value.clone().into()],
            ))
            .one(db)
            .await
            .map_err(crate::error::ApiError::internal_database_failure)?;
            Ok(row.map(|row| row.form_id))
        }
        _ => Ok(None),
    }
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
