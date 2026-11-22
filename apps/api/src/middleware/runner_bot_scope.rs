//! What the token of a runner bot may reach.
//!
//! A bot that a project bot connection made (it has a `bot_runners` row) holds a token that lives
//! on a host running command-line agents over issue text. The token is limited to the projects
//! where the bot has an enabled connection: every route is classified in
//! [`RUNNER_BOT_ROUTES`](super::runner_bot_routes::RUNNER_BOT_ROUTES), and a request is allowed
//! only by its class.
//!
//! * `NoProject`: the route names no project and its handler limits the bot to what it owns (the
//!   task protocol, whose handlers require the bot to be the task's participant, and catalogues);
//! * `Project(find)`: the route's target resolves to a project, found the way the permission layer
//!   finds it for that route family (the project in the path, an issue, a form, a record, a Flow
//!   object, a proposal, ...); allowed only when that project has an enabled connection for the
//!   bot, refused when the target is missing, unresolvable or in another project;
//! * `Own`: the resource belongs to the bot itself (its notifications, its uploads, its personal
//!   Flow space, a proposal it authored that no project claims yet); the handler filters by the
//!   caller;
//! * `Admin`: workspace administration (members, roles, bots, tokens, webhooks, connections and
//!   execution settings, workspace settings, project creation and deletion, alert sources and
//!   rules, ...): refused;
//! * `Refused`: everything else that spans projects or belongs to none (workspace-wide listings,
//!   workspace-scoped Flow, trust scores, ...): refused.
//!
//! A route that is not in the table is refused, so a new route is closed to a runner bot until it
//! is classified on purpose; the test `every_route_of_main_is_classified` fails until then. All
//! refusals answer one 403 that does not say which part failed. A bot without a `bot_runners` row is
//! not touched.
use axum::{
    body::Body,
    extract::{FromRequestParts, MatchedPath, RawPathParams},
    http::request::Parts,
};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use crate::error::ApiError;
use crate::oauth::delegated::parse_identifier;
use crate::services::ai_task_service::runner_bot_block;

pub use super::runner_bot_routes::RUNNER_BOT_ROUTES;

/// How the project of a request's target is found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    /// The path parameter is the project id.
    Project(&'static str),
    Issue(&'static str),
    /// `PREFIX-N` within the bot's workspace.
    IssueIdentifier(&'static str),
    Comment(&'static str),
    /// A table with `id` and `project_id` columns; the path parameter is the row's id.
    Table(&'static str, &'static str),
    /// A governance proposal: the one project its linked issues belong to, or, when it is linked to
    /// none, itself when the bot authored it.
    Proposal(&'static str),
    ProposalComment(&'static str),
    Decision(&'static str),
    /// A Flow object: its project, or, outside any project, itself when it is in the bot's own
    /// personal space.
    FlowObject(&'static str),
    /// A Flow scope: allowed only when it is the bot's own personal scope.
    FlowScope(&'static str),
    /// A row of `table` (key column `id_col`) that names a Flow object in `object_col`.
    FlowVia(&'static str, &'static str, &'static str, &'static str),
    /// The project is named in the JSON body: `project_id`, else the parent object, the conversion's
    /// source object or its preview, else the scope.
    Body,
    /// The project is the `project_id` query parameter; a request without it is refused.
    Query(&'static str),
}

/// What a route is to a runner bot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    NoProject,
    Project(Find),
    Own,
    Admin,
    Refused,
}

impl Class {
    /// The name used in documents and test output.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoProject => "no project",
            Self::Project(_) => "project-resolved",
            Self::Own => "own resource",
            Self::Admin => "admin",
            Self::Refused => "refused",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RunnerRoute {
    pub method: &'static str,
    /// The router's matched path template, exactly as registered in `main.rs`.
    pub pattern: &'static str,
    pub class: Class,
}

/// The entry for a method and the router's matched path pattern.
#[must_use]
pub fn lookup(method: &str, pattern: &str) -> Option<&'static RunnerRoute> {
    RUNNER_BOT_ROUTES
        .iter()
        .find(|entry| entry.method == method && entry.pattern == pattern)
}

/// What every refusal says, whether the route is closed, or the project is not connected, enabled
/// or does not exist: it never tells which.
pub const REFUSAL: &str = "this bot may act only in projects it is connected to";

/// The largest body read to find the project of a request that names it there.
const MAX_BODY_FOR_TARGET: usize = 2 * 1024 * 1024;

/// What a target resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Project {
        workspace_id: Uuid,
        project_id: Uuid,
    },
    /// The bot's own resource that belongs to no project.
    Own,
    /// No such target, or one that belongs to neither a project nor the bot.
    None,
}

#[derive(FromQueryResult)]
struct Author {
    author_id: String,
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the columns of the query")]
struct FlowObjectRow {
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    owner_user_id: Option<Uuid>,
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[allow(
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn,
    reason = "called with a fresh query result"
)]
fn project_target(row: Option<ProjectRow>) -> Target {
    match row {
        Some(ProjectRow {
            workspace_id,
            project_id: Some(project_id),
        }) => Target::Project {
            workspace_id,
            project_id,
        },
        _ => Target::None,
    }
}

async fn one<T: FromQueryResult>(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<T>, ApiError> {
    T::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(db)
        .await
        .map_err(ApiError::internal_database_failure)
}

async fn flow_object(db: &DatabaseConnection, object_id: &str, bot_id: Uuid) -> Result<Target, ApiError> {
    let row: Option<FlowObjectRow> = one(
        db,
        "SELECT o.workspace_id, o.project_id, s.owner_user_id \
         FROM flow_objects o JOIN flow_scopes s ON s.id = o.scope_id WHERE o.id::text = $1",
        vec![object_id.to_string().into()],
    )
    .await?;
    Ok(match row {
        Some(FlowObjectRow {
            workspace_id: Some(workspace_id),
            project_id: Some(project_id),
            ..
        }) => Target::Project {
            workspace_id,
            project_id,
        },
        Some(FlowObjectRow {
            project_id: None,
            owner_user_id: Some(owner),
            ..
        }) if owner == bot_id => Target::Own,
        _ => Target::None,
    })
}

async fn flow_scope(db: &DatabaseConnection, scope_id: &str, bot_id: Uuid) -> Result<Target, ApiError> {
    #[derive(FromQueryResult)]
    struct Owner {
        owner_user_id: Option<Uuid>,
    }
    let row: Option<Owner> = one(
        db,
        "SELECT owner_user_id FROM flow_scopes WHERE id::text = $1",
        vec![scope_id.to_string().into()],
    )
    .await?;
    Ok(if row.and_then(|row| row.owner_user_id) == Some(bot_id) {
        Target::Own
    } else {
        Target::None
    })
}

async fn proposal(db: &DatabaseConnection, proposal_id: &str, bot_id: Uuid) -> Result<Target, ApiError> {
    #[derive(FromQueryResult)]
    struct Linked {
        workspace_id: Uuid,
        project_id: Uuid,
    }
    let linked = Linked::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT DISTINCT p.workspace_id, wi.project_id \
         FROM proposal_issue_links pil JOIN work_items wi ON wi.id = pil.issue_id \
         JOIN projects p ON p.id = wi.project_id WHERE pil.proposal_id = $1",
        vec![proposal_id.to_string().into()],
    ))
    .all(db)
    .await
    .map_err(ApiError::internal_database_failure)?;
    if let [only] = linked.as_slice() {
        return Ok(Target::Project {
            workspace_id: only.workspace_id,
            project_id: only.project_id,
        });
    }
    if !linked.is_empty() {
        return Ok(Target::None);
    }
    let author: Option<Author> = one(
        db,
        "SELECT author_id FROM proposals WHERE id = $1",
        vec![proposal_id.to_string().into()],
    )
    .await?;
    Ok(if author.is_some_and(|row| row.author_id == bot_id.to_string()) {
        Target::Own
    } else {
        Target::None
    })
}

/// Finds the target of a request.
pub async fn resolve(
    db: &DatabaseConnection,
    find: Find,
    params: &[(String, String)],
    query: Option<&str>,
    body: Option<&serde_json::Value>,
    bot_id: Uuid,
    workspace_id: Uuid,
) -> Result<Target, ApiError> {
    let Some(first) = (match find {
        Find::Project(name)
        | Find::Issue(name)
        | Find::IssueIdentifier(name)
        | Find::Comment(name)
        | Find::Table(_, name)
        | Find::Proposal(name)
        | Find::ProposalComment(name)
        | Find::Decision(name)
        | Find::FlowObject(name)
        | Find::FlowScope(name)
        | Find::FlowVia(_, _, _, name) => param(params, name).map(str::to_owned),
        Find::Query(name) => query_value(query, name),
        Find::Body => Some(String::new()),
    }) else {
        return Ok(Target::None);
    };
    match find {
        Find::Project(_) => Ok(project_target(
            one(
                db,
                "SELECT workspace_id, id AS project_id FROM projects WHERE id::text = $1",
                vec![first.into()],
            )
            .await?,
        )),
        Find::Query(_) => {
            // The same row, found by the query value.
            Ok(project_target(
                one(
                    db,
                    "SELECT workspace_id, id AS project_id FROM projects WHERE id::text = $1",
                    vec![first.into()],
                )
                .await?,
            ))
        }
        Find::Issue(_) => Ok(project_target(
            one(
                db,
                "SELECT p.workspace_id, w.project_id FROM work_items w JOIN projects p ON p.id = w.project_id \
                 WHERE w.id::text = $1",
                vec![first.into()],
            )
            .await?,
        )),
        Find::IssueIdentifier(_) => {
            let Some((prefix, sequence)) = parse_identifier(&first) else {
                return Ok(Target::None);
            };
            Ok(project_target(
                one(
                    db,
                    "SELECT p.workspace_id, wi.project_id FROM work_items wi JOIN projects p ON p.id = wi.project_id \
                     WHERE p.workspace_id = $1 AND UPPER(p.key) = UPPER($2) AND wi.sequence_number = $3",
                    vec![workspace_id.into(), prefix.to_string().into(), sequence.into()],
                )
                .await?,
            ))
        }
        Find::Comment(_) => Ok(project_target(
            one(
                db,
                "SELECT p.workspace_id, w.project_id FROM comments c JOIN work_items w ON w.id = c.work_item_id \
                 JOIN projects p ON p.id = w.project_id WHERE c.id::text = $1",
                vec![first.into()],
            )
            .await?,
        )),
        Find::Table(table, _) => Ok(project_target(
            one(
                db,
                &format!(
                    "SELECT p.workspace_id, p.id AS project_id FROM {table} x JOIN projects p ON p.id = x.project_id \
                     WHERE x.id::text = $1"
                ),
                vec![first.into()],
            )
            .await?,
        )),
        Find::Proposal(_) => proposal(db, &first, bot_id).await,
        Find::ProposalComment(_) => {
            #[derive(FromQueryResult)]
            struct Owner {
                proposal_id: String,
            }
            let row: Option<Owner> = one(
                db,
                "SELECT proposal_id FROM proposal_comments WHERE id::text = $1",
                vec![first.into()],
            )
            .await?;
            match row {
                Some(row) => proposal(db, &row.proposal_id, bot_id).await,
                None => Ok(Target::None),
            }
        }
        Find::Decision(_) => {
            #[derive(FromQueryResult)]
            struct Owner {
                proposal_id: String,
            }
            let row: Option<Owner> = one(
                db,
                "SELECT proposal_id FROM decisions WHERE id::text = $1",
                vec![first.into()],
            )
            .await?;
            match row {
                Some(row) => proposal(db, &row.proposal_id, bot_id).await,
                None => Ok(Target::None),
            }
        }
        Find::FlowObject(_) => flow_object(db, &first, bot_id).await,
        Find::FlowScope(_) => flow_scope(db, &first, bot_id).await,
        Find::FlowVia(table, id_col, object_col, _) => {
            #[derive(FromQueryResult)]
            struct Object {
                object_id: Option<Uuid>,
            }
            let row: Option<Object> = one(
                db,
                &format!("SELECT {object_col} AS object_id FROM {table} WHERE {id_col}::text = $1"),
                vec![first.into()],
            )
            .await?;
            match row.and_then(|row| row.object_id) {
                Some(object) => flow_object(db, &object.to_string(), bot_id).await,
                None => Ok(Target::None),
            }
        }
        Find::Body => {
            let field = |name: &str| body.and_then(|body| body.get(name)).and_then(serde_json::Value::as_str);
            if let Some(project) = field("project_id") {
                return Ok(project_target(
                    one(
                        db,
                        "SELECT workspace_id, id AS project_id FROM projects WHERE id::text = $1",
                        vec![project.to_string().into()],
                    )
                    .await?,
                ));
            }
            if let Some(parent) = field("parent_object_id") {
                return flow_object(db, parent, bot_id).await;
            }
            if let Some(source) = field("source_object_id") {
                return flow_object(db, source, bot_id).await;
            }
            if let Some(preview) = field("preview_id") {
                return Box::pin(resolve(
                    db,
                    Find::FlowVia("flow_conversion_previews", "id", "source_object_id", "preview_id"),
                    &[("preview_id".to_string(), preview.to_string())],
                    None,
                    None,
                    bot_id,
                    workspace_id,
                ))
                .await;
            }
            if let Some(scope) = field("scope_id") {
                return flow_scope(db, scope, bot_id).await;
            }
            Ok(Target::None)
        }
    }
}

fn query_value(query: Option<&str>, name: &str) -> Option<String> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

/// Whether the runner bot `bot_id` of `workspace_id` may make this request; returns the body to
/// hand on (re-buffered when it had to be read to find the target). `Ok` for a bot that is not a
/// runner bot.
pub async fn authorize(
    db: &DatabaseConnection,
    bot_id: Uuid,
    workspace_id: Uuid,
    is_runner_bot: bool,
    method: &str,
    parts: &mut Parts,
    body: Body,
) -> Result<Body, ApiError> {
    if !is_runner_bot {
        return Ok(body);
    }
    let refuse = || ApiError::Forbidden(REFUSAL.to_string());
    let matched = parts
        .extensions
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let Some(entry) = matched.as_deref().and_then(|pattern| lookup(method, pattern)) else {
        return Err(refuse());
    };
    let find = match entry.class {
        Class::NoProject | Class::Own => return Ok(body),
        Class::Admin | Class::Refused => return Err(refuse()),
        Class::Project(find) => find,
    };
    let (body, json) = if find == Find::Body {
        let bytes = axum::body::to_bytes(body, MAX_BODY_FOR_TARGET)
            .await
            .map_err(|_| refuse())?;
        let json = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
        (Body::from(bytes), json)
    } else {
        (body, None)
    };
    let params: Vec<(String, String)> = RawPathParams::from_request_parts(parts, &())
        .await
        .map(|raw| {
            raw.iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let query = parts.uri.query().map(str::to_owned);
    match resolve(db, find, &params, query.as_deref(), json.as_ref(), bot_id, workspace_id).await? {
        Target::Own => Ok(body),
        Target::Project {
            workspace_id: target_workspace,
            project_id,
        } if target_workspace == workspace_id && runner_bot_block(db, project_id, bot_id).await?.is_none() => Ok(body),
        _ => Err(refuse()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{Class, Find, RUNNER_BOT_ROUTES, lookup};
    use std::collections::{BTreeMap, BTreeSet};

    /// Every `(method, pattern)` that `main.rs` registers behind `bot_or_user_auth_middleware`.
    fn routes_of_main() -> BTreeSet<(String, String)> {
        let source = include_str!("../main.rs");
        let bytes = source.as_bytes();
        let mut found = BTreeSet::new();
        let mut from = 0;
        while let Some(at) = source[from..].find(".route(") {
            let open = from + at + ".route(".len();
            from = open;
            let rest = source[open..].trim_start();
            let Some(literal) = rest.strip_prefix('"') else {
                continue;
            };
            let Some(end) = literal.find('"') else {
                continue;
            };
            let pattern = &literal[..end];
            // The body of this `.route(...)` call: up to its matching parenthesis.
            let mut depth = 1_i32;
            let mut index = open;
            while depth > 0 && index < bytes.len() {
                match bytes[index] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                index += 1;
            }
            let body = &source[open..index];
            if !body.contains("bot_or_user_auth_middleware") {
                continue;
            }
            for method in ["get", "post", "put", "patch", "delete"] {
                let needle = format!("{method}(");
                let mut search = 0;
                while let Some(position) = body[search..].find(&needle) {
                    let absolute = search + position;
                    let before = body[..absolute].chars().next_back();
                    search = absolute + needle.len();
                    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                        continue;
                    }
                    found.insert((method.to_ascii_uppercase(), pattern.to_string()));
                }
            }
        }
        found
    }

    /// A route cannot be added to `main.rs` without being classified here, and nothing in the table
    /// is a route that no longer exists.
    #[test]
    fn every_route_of_main_is_classified() {
        let main = routes_of_main();
        assert!(main.len() > 400, "the route scan found only {} routes", main.len());
        let table: BTreeSet<(String, String)> = RUNNER_BOT_ROUTES
            .iter()
            .map(|entry| (entry.method.to_string(), entry.pattern.to_string()))
            .collect();
        assert_eq!(
            table.len(),
            RUNNER_BOT_ROUTES.len(),
            "a method and pattern is listed twice"
        );
        let unclassified: Vec<_> = main.difference(&table).collect();
        assert!(
            unclassified.is_empty(),
            "routes without a runner bot class: {unclassified:#?}"
        );
        let stale: Vec<_> = table.difference(&main).collect();
        assert!(
            stale.is_empty(),
            "classified routes that main.rs does not register: {stale:#?}"
        );
    }

    /// The class of the routes that decide the shape of the rule, asserted one by one, and the
    /// count per class (printed with `--nocapture`).
    #[test]
    fn the_classes_of_the_routes_that_matter() {
        use Class::{Admin, NoProject, Own, Refused};
        let class = |method: &str, pattern: &str| {
            lookup(method, pattern)
                .unwrap_or_else(|| panic!("{method} {pattern} is not classified"))
                .class
        };
        let project = |class: Class| matches!(class, Class::Project(_));
        // No project: the task protocol and catalogues.
        for (method, pattern) in [
            ("POST", "/api/v1/ai/tasks/{task_id}/complete"),
            ("GET", "/api/v1/ai/tasks/active"),
            ("PUT", "/api/v1/ai/runner/report"),
            ("GET", "/api/v1/project-types"),
        ] {
            assert_eq!(class(method, pattern), NoProject, "{method} {pattern}");
        }
        // Own: notifications, uploads, the personal Flow home.
        for (method, pattern) in [
            ("GET", "/api/v1/notifications"),
            ("POST", "/api/v1/upload"),
            ("GET", "/api/v2/flow/home"),
            ("GET", "/api/v1/auth/me"),
        ] {
            assert_eq!(class(method, pattern), Own, "{method} {pattern}");
        }
        // Admin: workspace administration, connections, alert sources and rules, creation and deletion.
        for (method, pattern) in [
            ("POST", "/api/v1/workspaces/{workspace_id}/members"),
            ("PATCH", "/api/v1/workspaces/{workspace_id}/members/{user_id}"),
            ("POST", "/api/v1/workspaces/{workspace_id}/bots"),
            ("POST", "/api/v1/workspaces/{workspace_id}/webhooks"),
            ("POST", "/api/v1/workspaces/{workspace_id}/projects"),
            ("DELETE", "/api/v1/projects/{id}"),
            ("POST", "/api/v1/projects/{project_id}/bot-connection"),
            ("GET", "/api/v1/projects/{project_id}/execution-settings"),
            ("POST", "/api/v1/projects/{project_id}/inbound-sources"),
            ("POST", "/api/v1/projects/{project_id}/alert-rules"),
            ("PUT", "/api/v1/workspaces/{workspace_id}/features/flow"),
        ] {
            assert_eq!(class(method, pattern), Admin, "{method} {pattern}");
        }
        // Refused: workspace-wide listings and anything that belongs to no project.
        for (method, pattern) in [
            ("GET", "/api/v1/workspaces/{workspace_id}/projects"),
            ("GET", "/api/v1/proposals"),
            ("GET", "/api/v1/my/issues"),
            ("GET", "/api/v2/flow/block-share-placements"),
            ("POST", "/api/v2/flow/collab/tickets"),
            ("GET", "/api/v1/workspaces/{workspace_id}/flow/projection-lag"),
            ("GET", "/api/v1/workspaces/{workspace_id}/flow/navigator"),
            ("GET", "/api/v2/flow/workspaces/{workspace_id}/scope"),
        ] {
            assert_eq!(class(method, pattern), Refused, "{method} {pattern}");
        }
        // Project-resolved: every route whose target is a project's resource.
        for (method, pattern) in [
            ("GET", "/api/v1/projects/{id}"),
            ("POST", "/api/v1/projects/{project_id}/issues"),
            ("POST", "/api/v1/issues/{issue_id}/comments"),
            ("PATCH", "/api/v1/forms/{form_id}"),
            ("POST", "/api/v1/form-records/{record_id}/comments"),
            ("POST", "/api/v1/proposals/{id}/votes"),
            ("GET", "/api/v1/projects/{project_id}/board"),
            ("POST", "/api/v2/flow/objects/{object_id}/commands"),
            ("POST", "/api/v2/flow/objects"),
            ("POST", "/api/v1/flow/conversions"),
            ("GET", "/api/v2/flow/records/{record_id}/context"),
            ("GET", "/api/v1/search"),
        ] {
            assert!(project(class(method, pattern)), "{method} {pattern}");
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for entry in RUNNER_BOT_ROUTES {
            *counts.entry(entry.class.name()).or_default() += 1;
        }
        // The count per class is part of the review of a change to the table.
        assert_eq!(
            counts,
            BTreeMap::from([
                ("no project", 14),
                ("project-resolved", 294),
                ("own resource", 13),
                ("admin", 86),
                ("refused", 37),
            ]),
            "the classes of the {} routes changed",
            RUNNER_BOT_ROUTES.len()
        );
        for name in ["no project", "project-resolved", "own resource", "admin", "refused"] {
            assert!(counts.get(name).copied().unwrap_or(0) > 0, "no route is {name}");
        }
        // The workspace Flow reads that take the project from the query: allowed only for a named project.
        for pattern in [
            "/api/v1/workspaces/{workspace_id}/flow/objects",
            "/api/v1/workspaces/{workspace_id}/flow/search",
            "/api/v1/workspaces/{workspace_id}/flow/trash",
        ] {
            assert_eq!(
                class("GET", pattern),
                Class::Project(Find::Query("project_id")),
                "GET {pattern}"
            );
        }
        // Every resolver the table uses is exercised by the database test; list them here so a new one
        // is noticed.
        let finds: BTreeSet<String> = RUNNER_BOT_ROUTES
            .iter()
            .filter_map(|entry| match entry.class {
                Class::Project(find) => Some(match find {
                    Find::Table(table, _) => format!("table {table}"),
                    Find::FlowVia(table, ..) => format!("flow via {table}"),
                    other => format!("{other:?}").split('(').next().unwrap_or_default().to_string(),
                }),
                _ => None,
            })
            .collect();
        assert!(finds.contains("Body") && finds.contains("Proposal") && finds.contains("FlowObject"));
    }
}
