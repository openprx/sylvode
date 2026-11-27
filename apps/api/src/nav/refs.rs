//! The stored objects a navigation document names: loading them and placing each one relative to
//! the scope of the document (found, gone, or in another scope).

use std::collections::{HashMap, HashSet};

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::doc::{NavDoc, NavError, NavScope, Target};
use crate::error::ApiError;

pub struct ProjectRef {
    pub workspace_id: Uuid,
    pub name: String,
    pub type_key: String,
}

pub struct FormRef {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
}

pub struct FlowRef {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub archived: bool,
}

pub struct LayoutRef {
    pub workspace_id: Uuid,
    pub scope_kind: String,
    pub scope_id: Uuid,
    pub key: String,
    pub active: bool,
    /// Owned by a package installation; such layouts are never shown here.
    pub managed: bool,
    pub spec: Value,
}

/// Everything a document names, by object id.
#[derive(Default)]
pub struct Refs {
    pub projects: HashMap<Uuid, ProjectRef>,
    pub forms: HashMap<Uuid, FormRef>,
    pub flows: HashMap<Uuid, FlowRef>,
    pub layouts: HashMap<Uuid, LayoutRef>,
}

/// The scope a document belongs to.
#[derive(Debug, Clone, Copy)]
pub struct Where {
    pub scope: NavScope,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
}

/// A target placed against its scope.
#[derive(Debug, PartialEq, Eq)]
pub enum Place {
    /// The object exists in scope (and is not archived or package-owned).
    Found {
        name: String,
        href: String,
    },
    Missing,
    OutOfScope,
}

fn base(workspace_id: Uuid) -> String {
    format!("/workspace/{workspace_id}")
}

fn project_base(workspace_id: Uuid, project_id: Uuid) -> String {
    format!("{}/projects/{project_id}", base(workspace_id))
}

/// Places a target that names a stored object. Targets that name none are `None`.
#[must_use]
pub fn locate(target: &Target, at: &Where, refs: &Refs) -> Option<Place> {
    let place = match target {
        Target::Project { project_id } => match refs.projects.get(project_id) {
            None => Place::Missing,
            Some(project) if project.workspace_id != at.workspace_id || at.scope != NavScope::Workspace => {
                Place::OutOfScope
            }
            Some(project) => {
                let mut href = project_base(at.workspace_id, *project_id);
                if project.type_key == "custom_form" {
                    href.push_str("/forms");
                }
                Place::Found {
                    name: project.name.clone(),
                    href,
                }
            }
        },
        Target::Form { form_id } => match refs.forms.get(form_id) {
            None => Place::Missing,
            Some(form) if form.workspace_id != at.workspace_id => Place::OutOfScope,
            Some(form) if at.project_id.is_some_and(|project| project != form.project_id) => Place::OutOfScope,
            Some(form) => Place::Found {
                name: form.name.clone(),
                href: format!(
                    "{}/forms?form={form_id}",
                    project_base(at.workspace_id, form.project_id)
                ),
            },
        },
        Target::FlowPage { object_id } => match refs.flows.get(object_id) {
            None => Place::Missing,
            Some(flow) if flow.workspace_id != at.workspace_id => Place::OutOfScope,
            Some(flow) if flow.project_id.is_some() && flow.project_id != at.project_id => Place::OutOfScope,
            Some(flow) if flow.archived => Place::Missing,
            Some(flow) => Place::Found {
                name: flow.title.clone(),
                href: format!("{}/flow/{object_id}", base(at.workspace_id)),
            },
        },
        Target::LayoutPage { layout_id } => match refs.layouts.get(layout_id) {
            None => Place::Missing,
            Some(layout) if layout.workspace_id != at.workspace_id => Place::OutOfScope,
            Some(layout) if !layout.active || layout.managed => Place::Missing,
            Some(layout) => {
                let in_scope = match layout.scope_kind.as_str() {
                    "workspace" => layout.scope_id == at.workspace_id,
                    "project" => at.project_id == Some(layout.scope_id),
                    _ => false,
                };
                if in_scope {
                    let root = at.project_id.map_or_else(
                        || base(at.workspace_id),
                        |project| project_base(at.workspace_id, project),
                    );
                    Place::Found {
                        name: layout.key.clone(),
                        href: format!("{root}/pages/{layout_id}"),
                    }
                } else {
                    Place::OutOfScope
                }
            }
        },
        Target::Builtin { .. } | Target::PluginApp { .. } | Target::External { .. } => return None,
    };
    Some(place)
}

/// Write-time check of every entry that names an object: it exists, lies in the scope of the
/// document, and a layout page fits the `page` slot. Reads nothing of the writer's own access.
///
/// # Errors
/// `nav_invalid` with reason `target_not_found` or `target_out_of_scope` and the entry's path.
pub fn validate_targets(doc: &NavDoc, at: &Where, refs: &Refs) -> Result<(), NavError> {
    for (s, section) in doc.sections.iter().enumerate() {
        for (e, entry) in section.entries.iter().enumerate() {
            let path = format!("/sections/{s}/entries/{e}/target");
            match locate(&entry.target, at, refs) {
                None | Some(Place::Found { .. }) => {}
                Some(Place::Missing) => return Err(NavError::invalid(path, "target_not_found")),
                Some(Place::OutOfScope) => return Err(NavError::invalid(path, "target_out_of_scope")),
            }
            if let Target::LayoutPage { layout_id } = &entry.target
                && let Some(layout) = refs.layouts.get(layout_id)
                && !fits_page_slot(&layout.spec)
            {
                return Err(NavError::invalid(path, "target_out_of_scope"));
            }
        }
    }
    Ok(())
}

fn fits_page_slot(spec: &Value) -> bool {
    let Some(slot) = sv_layout::slot("page") else {
        return false;
    };
    let Ok(bytes) = serde_json::to_vec(spec) else {
        return false;
    };
    sv_layout::validate_bytes(
        &bytes,
        &sv_layout::Options {
            slot: Some(&slot),
            context: sv_layout::Context::Console,
        },
    )
    .is_ok()
}

fn ids_of(doc: &NavDoc, pick: impl Fn(&Target) -> Option<Uuid>) -> Vec<Uuid> {
    let set: HashSet<Uuid> = doc.entries().filter_map(|(_, entry)| pick(&entry.target)).collect();
    set.into_iter().collect()
}

#[derive(FromQueryResult)]
struct ProjectRow {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    type_key: String,
}

#[derive(FromQueryResult)]
struct FormRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    name: String,
}

#[derive(FromQueryResult)]
struct FlowRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    title: String,
    archived: bool,
}

#[derive(FromQueryResult)]
struct LayoutRow {
    id: Uuid,
    workspace_id: Uuid,
    scope_kind: String,
    scope_id: Uuid,
    layout_key: String,
    status: String,
    installation_id: Option<Uuid>,
    spec: Value,
}

async fn rows<T: FromQueryResult>(db: &impl ConnectionTrait, sql: &str, ids: Vec<Uuid>) -> Result<Vec<T>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(T::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![ids.into()],
    ))
    .all(db)
    .await?)
}

/// Loads every object the document names, wherever it lives; [`locate`] decides what is in scope.
///
/// # Errors
/// A database failure.
pub async fn load(db: &impl ConnectionTrait, doc: &NavDoc) -> Result<Refs, ApiError> {
    let mut refs = Refs::default();
    let projects = ids_of(doc, |t| match t {
        Target::Project { project_id } => Some(*project_id),
        _ => None,
    });
    for row in rows::<ProjectRow>(
        db,
        "SELECT id, workspace_id, name, COALESCE(type_key, 'code_project') AS type_key FROM projects WHERE id = ANY($1)",
        projects,
    )
    .await?
    {
        refs.projects.insert(
            row.id,
            ProjectRef {
                workspace_id: row.workspace_id,
                name: row.name,
                type_key: row.type_key,
            },
        );
    }
    let forms = ids_of(doc, |t| match t {
        Target::Form { form_id } => Some(*form_id),
        _ => None,
    });
    for row in rows::<FormRow>(
        db,
        "SELECT id, workspace_id, project_id, name FROM project_forms WHERE id = ANY($1)",
        forms,
    )
    .await?
    {
        refs.forms.insert(
            row.id,
            FormRef {
                workspace_id: row.workspace_id,
                project_id: row.project_id,
                name: row.name,
            },
        );
    }
    let flows = ids_of(doc, |t| match t {
        Target::FlowPage { object_id } => Some(*object_id),
        _ => None,
    });
    for row in rows::<FlowRow>(
        db,
        "SELECT o.id, o.workspace_id, o.project_id, COALESCE(p.title, '') AS title, \
                (o.lifecycle_status <> 'active') AS archived \
         FROM flow_objects o LEFT JOIN flow_object_projections p ON p.object_id = o.id \
         WHERE o.id = ANY($1) AND o.object_type = 'page'",
        flows,
    )
    .await?
    {
        refs.flows.insert(
            row.id,
            FlowRef {
                workspace_id: row.workspace_id,
                project_id: row.project_id,
                title: row.title,
                archived: row.archived,
            },
        );
    }
    let layouts = ids_of(doc, |t| match t {
        Target::LayoutPage { layout_id } => Some(*layout_id),
        _ => None,
    });
    for row in rows::<LayoutRow>(
        db,
        "SELECT id, workspace_id, scope_kind, scope_id, layout_key, status, installation_id, spec \
         FROM layouts WHERE id = ANY($1)",
        layouts,
    )
    .await?
    {
        refs.layouts.insert(
            row.id,
            LayoutRef {
                workspace_id: row.workspace_id,
                scope_kind: row.scope_kind,
                scope_id: row.scope_id,
                key: row.layout_key,
                active: row.status == "active",
                managed: row.installation_id.is_some(),
                spec: row.spec,
            },
        );
    }
    Ok(refs)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid, Uuid, Uuid) {
        (
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            Uuid::from_u128(4),
        )
    }

    #[test]
    fn a_form_must_belong_to_the_project_of_a_project_navigation() {
        let (workspace, project, other, form) = ids();
        let mut refs = Refs::default();
        refs.forms.insert(
            form,
            FormRef {
                workspace_id: workspace,
                project_id: other,
                name: "Intake".into(),
            },
        );
        let target = Target::Form { form_id: form };
        let at_project = Where {
            scope: NavScope::Project,
            workspace_id: workspace,
            project_id: Some(project),
        };
        assert_eq!(locate(&target, &at_project, &refs), Some(Place::OutOfScope));
        let at_other = Where {
            project_id: Some(other),
            ..at_project
        };
        assert!(matches!(locate(&target, &at_other, &refs), Some(Place::Found { .. })));
        let at_workspace = Where {
            scope: NavScope::Workspace,
            project_id: None,
            ..at_project
        };
        assert!(matches!(
            locate(&target, &at_workspace, &refs),
            Some(Place::Found { .. })
        ));
        let gone = Target::Form {
            form_id: Uuid::from_u128(99),
        };
        assert_eq!(locate(&gone, &at_workspace, &refs), Some(Place::Missing));
    }

    #[test]
    fn a_layout_page_is_found_only_when_active_unmanaged_and_in_scope() {
        let (workspace, project, _, layout) = ids();
        let mut refs = Refs::default();
        let mut stored = LayoutRef {
            workspace_id: workspace,
            scope_kind: "project".into(),
            scope_id: project,
            key: "service.summary".into(),
            active: true,
            managed: false,
            spec: Value::Null,
        };
        let target = Target::LayoutPage { layout_id: layout };
        let at_project = Where {
            scope: NavScope::Project,
            workspace_id: workspace,
            project_id: Some(project),
        };
        let at_workspace = Where {
            scope: NavScope::Workspace,
            workspace_id: workspace,
            project_id: None,
        };
        refs.layouts.insert(layout, clone_layout(&stored));
        match locate(&target, &at_project, &refs).expect("a placement") {
            Place::Found { href, .. } => assert_eq!(
                href,
                format!("/workspace/{workspace}/projects/{project}/pages/{layout}")
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(locate(&target, &at_workspace, &refs), Some(Place::OutOfScope));
        stored.active = false;
        refs.layouts.insert(layout, clone_layout(&stored));
        assert_eq!(locate(&target, &at_project, &refs), Some(Place::Missing));
    }

    fn clone_layout(layout: &LayoutRef) -> LayoutRef {
        LayoutRef {
            workspace_id: layout.workspace_id,
            scope_kind: layout.scope_kind.clone(),
            scope_id: layout.scope_id,
            key: layout.key.clone(),
            active: layout.active,
            managed: layout.managed,
            spec: layout.spec.clone(),
        }
    }
}
