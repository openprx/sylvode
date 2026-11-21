//! What the token of a runner bot may reach.
//!
//! A bot that a project bot connection made (it has a `bot_runners` row) holds a token that lives
//! on a host running command-line agents over issue text. It is not a workspace-wide token: it
//! may call only the routes in [`RUNNER_BOT_ROUTES`], and a route that names a project or an issue
//! only for a project that has an enabled connection for this bot.
//!
//! This is the one place that rule is enforced; `bot_or_user_auth_middleware` calls
//! [`authorize`] for every bot request. A bot without a `bot_runners` row is not touched.
//! Anything that is not in the table is refused, so a new route is closed to a runner bot until it
//! is added here on purpose.
//!
//! The task-bound routes (`/ai/tasks/{task_id}...`, `/ai/callbacks/task/...`) need no project
//! here: their handlers already require the bot to be the task's participant, and a task that was
//! running when its connection was disabled must still be able to report how it ended.
use axum::{
    extract::{FromRequestParts, MatchedPath, RawPathParams},
    http::request::Parts,
};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::error::ApiError;
use crate::oauth::delegated::{Grantee, ProjectRef, Resolver, resolve_target};
use crate::services::ai_task_service::runner_bot_block;

/// What a route names that decides whether a runner bot may use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Reachable whatever the project: the route's handler limits the bot to what it owns.
    NoProject,
    /// Reachable only for a project the bot is connected to and has enabled. The resolver finds
    /// the project from the path.
    Project(Resolver),
}

#[derive(Clone, Copy, Debug)]
pub struct RunnerRoute {
    pub method: &'static str,
    /// The router's matched path template, exactly as registered in `main.rs`.
    pub pattern: &'static str,
    pub scope: Scope,
}

const fn route(method: &'static str, pattern: &'static str, scope: Scope) -> RunnerRoute {
    RunnerRoute { method, pattern, scope }
}

const NONE: Scope = Scope::NoProject;
const PROJECT: Scope = Scope::Project(Resolver::Project("project_id"));
const PROJECT_ID: Scope = Scope::Project(Resolver::Project("id"));
const ISSUE_ID: Scope = Scope::Project(Resolver::Issue("id"));
const ISSUE: Scope = Scope::Project(Resolver::Issue("issue_id"));

/// The routes a runner bot token may call. A method and pattern that is not here is refused.
pub static RUNNER_BOT_ROUTES: &[RunnerRoute] = &[
    // The task protocol: the runner reads the protocol, reports itself, and works its own tasks.
    route("GET", "/api/v1/ai/protocol", NONE),
    route("PUT", "/api/v1/ai/runner/report", NONE),
    route("GET", "/api/v1/ai/tasks/active", NONE),
    route("GET", "/api/v1/ai/tasks/{task_id}", NONE),
    route("POST", "/api/v1/ai/tasks/{task_id}/ack", NONE),
    route("POST", "/api/v1/ai/tasks/{task_id}/progress", NONE),
    route("POST", "/api/v1/ai/tasks/{task_id}/complete", NONE),
    route("POST", "/api/v1/ai/tasks/{task_id}/fail", NONE),
    route("POST", "/api/v1/ai/callbacks/task/{task_id}/complete", NONE),
    route("POST", "/api/v1/ai/callbacks/task/{task_id}/fail", NONE),
    route("POST", "/api/v1/ai/callbacks/task/{task_id}/progress", NONE),
    route("GET", "/api/v1/auth/me", NONE),
    // A project the bot is connected to.
    route("GET", "/api/v1/projects/{id}", PROJECT_ID),
    route("GET", "/api/v1/projects/{project_id}/context", PROJECT),
    route("GET", "/api/v1/projects/{project_id}/agent-policy", PROJECT),
    route("GET", "/api/v1/projects/{project_id}/ai/tasks", PROJECT),
    route("POST", "/api/v1/projects/{project_id}/ai/tasks", PROJECT),
    route("GET", "/api/v1/projects/{project_id}/issues", PROJECT),
    route("POST", "/api/v1/projects/{project_id}/issues", PROJECT),
    route("GET", "/api/v1/issues/{id}", ISSUE_ID),
    route("PUT", "/api/v1/issues/{id}", ISSUE_ID),
    route(
        "GET",
        "/api/v1/issues/by-identifier/{identifier}",
        Scope::Project(Resolver::IssueIdentifier("identifier")),
    ),
    route("GET", "/api/v1/issues/{issue_id}/comments", ISSUE),
    route("POST", "/api/v1/issues/{issue_id}/comments", ISSUE),
    route("GET", "/api/v1/issues/{issue_id}/activities", ISSUE),
    route("GET", "/api/v1/issues/{issue_id}/labels", ISSUE),
    route("POST", "/api/v1/issues/{issue_id}/labels/{label_id}", ISSUE),
    route("DELETE", "/api/v1/issues/{issue_id}/labels/{label_id}", ISSUE),
    route("GET", "/api/v1/issues/{issue_id}/ai-tasks", ISSUE),
];

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

/// Whether the runner bot `bot_id` of `workspace_id` may make this request. `Ok` for a bot that is
/// not a runner bot.
pub async fn authorize(
    db: &DatabaseConnection,
    bot_id: Uuid,
    workspace_id: Uuid,
    is_runner_bot: bool,
    method: &str,
    parts: &mut Parts,
) -> Result<(), ApiError> {
    if !is_runner_bot {
        return Ok(());
    }
    let refuse = || ApiError::Forbidden(REFUSAL.to_string());
    let matched = parts
        .extensions
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let Some(entry) = matched.as_deref().and_then(|pattern| lookup(method, pattern)) else {
        return Err(refuse());
    };
    let Scope::Project(resolver) = entry.scope else {
        return Ok(());
    };
    let params: Vec<(String, String)> = RawPathParams::from_request_parts(parts, &())
        .await
        .map(|raw| {
            raw.iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let target = resolve_target(
        db,
        resolver,
        &params,
        Grantee {
            user_id: bot_id,
            workspace_id,
        },
    )
    .await
    .map_err(ApiError::internal_database_failure)?;
    let Some(target) = target else {
        return Err(refuse());
    };
    let ProjectRef::Project(project_id) = target.project else {
        return Err(refuse());
    };
    if target.workspace_id != workspace_id || runner_bot_block(db, project_id, bot_id).await?.is_some() {
        return Err(refuse());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RUNNER_BOT_ROUTES, lookup};

    /// Every pattern in the table is a route `main.rs` registers, spelled exactly the same way, so a
    /// renamed route cannot silently stay in the table while its new spelling is refused.
    #[test]
    fn every_pattern_is_registered_in_main() {
        let main = include_str!("../main.rs");
        for entry in RUNNER_BOT_ROUTES {
            assert!(
                main.contains(&format!("\"{}\"", entry.pattern)),
                "{} is not a route of main.rs",
                entry.pattern
            );
        }
    }

    #[test]
    fn a_method_and_pattern_pair_is_listed_once() {
        for (index, entry) in RUNNER_BOT_ROUTES.iter().enumerate() {
            assert!(
                RUNNER_BOT_ROUTES
                    .iter()
                    .skip(index + 1)
                    .all(|other| (other.method, other.pattern) != (entry.method, entry.pattern)),
                "{} {} is listed twice",
                entry.method,
                entry.pattern
            );
        }
        assert!(lookup("DELETE", "/api/v1/projects/{id}").is_none());
    }
}
