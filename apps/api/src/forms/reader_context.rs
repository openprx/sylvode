//! Per-request reader context: builds the [`RecordReadPolicy`] of one role for each form it reads,
//! once per form, on the caller's connection or transaction.
//!
//! A relation value stores a snapshot of the target record's title as the writer saw it, and that
//! title can itself render a relation snapshot of a third form. Restriction is therefore
//! transitive: a relation field is restricted when its target form, or any form reachable from it
//! through relation fields, hides a field from this role. Exploration is breadth-first and capped
//! at [`MAX_EXPLORED_FORMS`]; everything that cannot be decided (an unresolvable relation config, a
//! missing form, a form beyond the cap) counts as restricted.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::{
    permissions::{
        denied_read_fields_from_policy, normalize_permission_subject_id, permission_policy_allows,
        permission_policy_record_scope, role_is_form_admin,
    },
    read_policy::RecordReadPolicy,
    relations::relation_target_config,
    schema::parse_fields,
};
use crate::error::ApiError;

/// Most forms one context explores while deciding relation restriction.
pub const MAX_EXPLORED_FORMS: usize = 64;

/// The parts of a form a read policy is built from.
#[derive(Debug, Clone, Copy)]
pub struct FormReadInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub title_template: &'a str,
    pub schema: &'a Value,
}

/// One explored form: whether its policy restricts the role (see [`node_restricts`]), and the
/// target form key of each of its relation fields (`None` when the relation config does not
/// resolve).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormNode {
    pub restricts: bool,
    pub edges: Vec<Option<String>>,
}

#[derive(Debug, FromQueryResult)]
struct NodeRow {
    schema: Value,
    policy: Option<Value>,
}

#[derive(Debug, FromQueryResult)]
struct FormRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    title_template: String,
    schema: Value,
}

/// Read policies of one role, cached per form for the lifetime of a request.
#[derive(Debug)]
pub struct ReaderContext {
    role: String,
    policies: HashMap<Uuid, Arc<RecordReadPolicy>>,
    nodes: HashMap<(Uuid, Uuid, String), Option<FormNode>>,
}

impl ReaderContext {
    #[must_use]
    pub fn new(role: &str) -> Self {
        Self {
            role: role.to_string(),
            policies: HashMap::new(),
            nodes: HashMap::new(),
        }
    }

    /// The read policy of this role on `form`.
    pub async fn policy<C: ConnectionTrait>(
        &mut self,
        conn: &C,
        form: FormReadInput<'_>,
    ) -> Result<Arc<RecordReadPolicy>, ApiError> {
        if let Some(policy) = self.policies.get(&form.id) {
            return Ok(Arc::clone(policy));
        }
        let policy = if role_is_form_admin(&self.role) {
            RecordReadPolicy::unrestricted(form.title_template.to_string())
        } else {
            self.build_policy(conn, form).await?
        };
        let policy = Arc::new(policy);
        self.policies.insert(form.id, Arc::clone(&policy));
        Ok(policy)
    }

    /// The read policy of this role on the form `form_id`; `Ok(None)` when the form row no longer
    /// exists.
    pub async fn policy_by_id<C: ConnectionTrait>(
        &mut self,
        conn: &C,
        form_id: Uuid,
    ) -> Result<Option<Arc<RecordReadPolicy>>, ApiError> {
        if let Some(policy) = self.policies.get(&form_id) {
            return Ok(Some(Arc::clone(policy)));
        }
        let Some(row) = FormRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, project_id, title_template, schema FROM project_forms WHERE id = $1",
            vec![form_id.into()],
        ))
        .one(conn)
        .await?
        else {
            return Ok(None);
        };
        let input = FormReadInput {
            id: row.id,
            workspace_id: row.workspace_id,
            project_id: row.project_id,
            title_template: &row.title_template,
            schema: &row.schema,
        };
        self.policy(conn, input).await.map(Some)
    }

    async fn build_policy<C: ConnectionTrait>(
        &mut self,
        conn: &C,
        form: FormReadInput<'_>,
    ) -> Result<RecordReadPolicy, ApiError> {
        let subject_id = normalize_permission_subject_id(&self.role)?;
        let own = NodeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT f.schema, p.policy FROM project_forms f \
             LEFT JOIN form_permissions p ON p.form_id = f.id AND p.subject_type = 'role' AND p.subject_id = $2 \
             WHERE f.id = $1",
            vec![form.id.into(), subject_id.clone().into()],
        ))
        .one(conn)
        .await?;
        let denied = own
            .as_ref()
            .and_then(|row| row.policy.as_ref())
            .map(denied_read_fields_from_policy)
            .unwrap_or_default();

        let fields = parse_fields(form.schema).map_err(ApiError::BadRequest)?;
        let start_edges = fields
            .iter()
            .filter(|field| field.field_type == "relation")
            .map(|field| (field.key.clone(), relation_edge(form.schema, &field.key)))
            .collect::<Vec<_>>();

        // Breadth-first exploration of the forms reachable through relation fields.
        let mut explored: BTreeMap<String, Option<FormNode>> = BTreeMap::new();
        let mut queue = start_edges
            .iter()
            .filter_map(|(_, edge)| edge.clone())
            .collect::<VecDeque<_>>();
        while let Some(form_key) = queue.pop_front() {
            if explored.contains_key(&form_key) {
                continue;
            }
            if explored.len() >= MAX_EXPLORED_FORMS {
                break;
            }
            let node = self
                .node(conn, form.workspace_id, form.project_id, &form_key, &subject_id)
                .await?;
            if let Some(node) = &node {
                queue.extend(node.edges.iter().flatten().cloned());
            }
            explored.insert(form_key, node);
        }
        let restricted_forms = restricted_form_keys(&explored);
        let restricted = start_edges
            .into_iter()
            .filter(|(_, edge)| edge_is_restricted(edge.as_deref(), &explored, &restricted_forms))
            .map(|(field_key, _)| field_key)
            .collect();
        Ok(RecordReadPolicy::new(
            form.title_template.to_string(),
            denied,
            restricted,
        ))
    }

    async fn node<C: ConnectionTrait>(
        &mut self,
        conn: &C,
        workspace_id: Uuid,
        project_id: Uuid,
        form_key: &str,
        subject_id: &str,
    ) -> Result<Option<FormNode>, ApiError> {
        let cache_key = (workspace_id, project_id, form_key.to_string());
        if let Some(node) = self.nodes.get(&cache_key) {
            return Ok(node.clone());
        }
        let row = NodeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT f.schema, p.policy FROM project_forms f \
             LEFT JOIN form_permissions p ON p.form_id = f.id AND p.subject_type = 'role' AND p.subject_id = $4 \
             WHERE f.workspace_id = $1 AND f.project_id = $2 AND f.key = $3",
            vec![
                workspace_id.into(),
                project_id.into(),
                form_key.to_string().into(),
                subject_id.to_string().into(),
            ],
        ))
        .one(conn)
        .await?;
        let node = row.map(|row| {
            let edges = parse_fields(&row.schema).map_or_else(
                // A schema that does not parse cannot be judged: one unresolvable edge restricts it.
                |_| vec![None],
                |fields| {
                    fields
                        .iter()
                        .filter(|field| field.field_type == "relation")
                        .map(|field| relation_edge(&row.schema, &field.key))
                        .collect()
                },
            );
            FormNode {
                restricts: node_restricts(row.policy.as_ref()),
                edges,
            }
        });
        self.nodes.insert(cache_key, node.clone());
        Ok(node)
    }
}

/// Whether a target form's policy row for the role restricts a relation snapshot of it.
///
/// A restriction is a read-denied field, a denied `form.view` (the reader may not see the form at
/// all), or a record scope limited to owned records (the reader may not see most of its records). No row means the permissive defaults, which restrict nothing.
#[must_use]
pub fn node_restricts(policy: Option<&Value>) -> bool {
    policy.is_some_and(|policy| {
        !denied_read_fields_from_policy(policy).is_empty()
            || !permission_policy_allows(policy, "form.view")
            || permission_policy_record_scope(policy) == "owned"
    })
}

fn relation_edge(schema: &Value, field_key: &str) -> Option<String> {
    relation_target_config(schema, Some(field_key), None)
        .ok()
        .map(|config| config.form_key)
}

fn edge_is_restricted(
    edge: Option<&str>,
    explored: &BTreeMap<String, Option<FormNode>>,
    restricted_forms: &BTreeSet<String>,
) -> bool {
    edge.is_none_or(|form_key| !explored.contains_key(form_key) || restricted_forms.contains(form_key))
}

/// The explored forms that restrict the reader, as a fixed point.
///
/// A form is restricted when it is missing, hides a field, has an unresolvable relation, or
/// relates to a missing, unexplored or restricted form. Cycles settle because a form only ever turns restricted.
pub fn restricted_form_keys(explored: &BTreeMap<String, Option<FormNode>>) -> BTreeSet<String> {
    let mut restricted = BTreeSet::new();
    loop {
        let mut changed = false;
        for (form_key, node) in explored {
            if restricted.contains(form_key) {
                continue;
            }
            let is_restricted = node.as_ref().is_none_or(|node| {
                node.restricts
                    || node
                        .edges
                        .iter()
                        .any(|edge| edge_is_restricted(edge.as_deref(), explored, &restricted))
            });
            if is_restricted {
                restricted.insert(form_key.clone());
                changed = true;
            }
        }
        if !changed {
            return restricted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FormNode, edge_is_restricted, node_restricts, restricted_form_keys};
    use serde_json::json;

    #[test]
    fn a_hidden_or_owned_scoped_target_form_restricts() {
        assert!(!node_restricts(None));
        assert!(!node_restricts(Some(&json!({}))));
        assert!(!node_restricts(Some(
            &json!({"actions": {"form.view": true}, "record_scope": "all"})
        )));
        assert!(node_restricts(Some(&json!({"fields": {"salary": {"read": false}}}))));
        assert!(node_restricts(Some(&json!({"actions": {"form.view": false}}))));
        assert!(node_restricts(Some(&json!({"record_scope": "owned"}))));
        assert!(node_restricts(Some(&json!({"record_scope": "own"}))));
    }
    use std::collections::BTreeMap;

    fn node(denied: bool, edges: &[Option<&str>]) -> FormNode {
        FormNode {
            restricts: denied,
            edges: edges.iter().map(|edge| edge.map(str::to_string)).collect(),
        }
    }

    fn graph(nodes: &[(&str, Option<FormNode>)]) -> BTreeMap<String, Option<FormNode>> {
        nodes
            .iter()
            .map(|(key, node)| ((*key).to_string(), node.clone()))
            .collect()
    }

    #[test]
    fn restriction_is_transitive_through_relation_chains() {
        // The start form A relates to B, B to C, and C denies a field.
        let explored = graph(&[("b", Some(node(false, &[Some("c")]))), ("c", Some(node(true, &[])))]);
        let restricted = restricted_form_keys(&explored);

        assert!(restricted.contains("b"));
        assert!(restricted.contains("c"));
        assert!(edge_is_restricted(Some("b"), &explored, &restricted));
    }

    #[test]
    fn a_cycle_with_nothing_denied_stays_unrestricted() {
        let explored = graph(&[
            ("a", Some(node(false, &[Some("b")]))),
            ("b", Some(node(false, &[Some("a")]))),
        ]);
        let restricted = restricted_form_keys(&explored);

        assert!(restricted.is_empty());
        assert!(!edge_is_restricted(Some("a"), &explored, &restricted));
    }

    #[test]
    fn unresolvable_missing_and_unexplored_edges_are_restricted() {
        let explored = graph(&[
            ("broken", Some(node(false, &[None]))),
            ("gone", None),
            ("far", Some(node(false, &[Some("beyond_the_cap")]))),
            ("clean", Some(node(false, &[]))),
        ]);
        let restricted = restricted_form_keys(&explored);

        assert!(restricted.contains("broken"));
        assert!(restricted.contains("gone"));
        assert!(restricted.contains("far"));
        assert!(!restricted.contains("clean"));
        assert!(edge_is_restricted(None, &explored, &restricted));
        assert!(edge_is_restricted(Some("beyond_the_cap"), &explored, &restricted));
    }
}
