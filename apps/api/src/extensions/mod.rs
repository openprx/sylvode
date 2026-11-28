//! Extension host v4: plugin invocation (contract C1 §3-§5).
//!
//! * [`host`]: the WebAssembly host. Runs a component under fuel, epoch, memory, host-call and
//!   process-wide limits and links only the interfaces a grant set can use.
//! * [`dispatch`]: a host call becomes an in-process request to the API router, so a plugin reaches
//!   exactly the handlers a user reaches, under the same checks.
//! * [`resource`]: resolves the target of an operation for the catalogue checker.
//! * [`install`]: installations, grants, review and approval, the invoke route and the live view the
//!   host loads before every host call. [`rules`]: plugin field rules for the form engine.
//! * This file: the marker that tells handlers a request comes from a plugin ([`current`]), the
//!   [`InternalCall`] request extension and its authentication ([`internal_claims`]).

use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, MatchedPath, RawPathParams, Request},
    middleware::Next,
    response::Response,
};
use platform::{
    app::AppState,
    auth::{JwtClaims, TokenType},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

use crate::catalogue::{InstallationView, Principal, Refusal, RefusalCode, allowed_pre, allowed_target, by_route};
use crate::error::ApiError;
use crate::middleware::scoped_bot::ProjectFilter;

#[cfg(test)]
mod body_ids_tests;
pub mod dispatch;
pub mod host;
pub mod install;
pub mod resource;
pub mod rules;

/// What a handler may learn about a plugin call; set by the internal-call branch of the
/// authentication middleware around the handler, and read through [`current`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginCall {
    pub installation_id: Uuid,
    pub generation: i32,
    /// `user` in the MVP.
    pub actor_kind: &'static str,
    pub actor_id: Uuid,
}

tokio::task_local! {
    static CURRENT: PluginCall;
}

/// The plugin call the current request belongs to, if any. A task-local: handlers rebuild their
/// own request extensions, so an extension marker would be lost on the way.
#[must_use]
pub fn current() -> Option<PluginCall> {
    CURRENT.try_with(|call| *call).ok()
}

/// The projects and forms a plugin call may reach beyond the one target the host resolved: the
/// grants of the capability being called, narrowed to the installation's project.
///
/// It is what a scoped bot's access row is for a bot (see `middleware::scoped_bot`): secondary ids
/// in a body or query and filtered lists are held to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCover {
    workspace_id: Uuid,
    /// The project of a project installation; `None` for a workspace installation.
    pinned: Option<Uuid>,
    /// `(project, form)` selectors of the grants of the capability; `None` = any.
    grants: Vec<(Option<Uuid>, Option<Uuid>)>,
}

impl PluginCover {
    /// The cover of `capability` for `installation`.
    #[must_use]
    pub fn new(installation: &InstallationView, capability: &str) -> Self {
        Self {
            workspace_id: installation.workspace_id,
            pinned: installation.project_id,
            grants: installation
                .grants
                .iter()
                .filter(|grant| grant.capability == capability)
                .map(|grant| (grant.project, grant.form))
                .collect(),
        }
    }

    /// Whether an object of `workspace_id`, in `project_id`, in `form_id` when it is a form or a
    /// form's child, is covered. Same rule as the host's check of the primary target.
    #[must_use]
    pub fn allows(&self, workspace_id: Uuid, project_id: Uuid, form_id: Option<Uuid>) -> bool {
        workspace_id == self.workspace_id
            && self.pinned.is_none_or(|pinned| pinned == project_id)
            && self.grants.iter().any(|(project, form)| {
                project.is_none_or(|project| project == project_id) && form.is_none_or(|form| form_id == Some(form))
            })
    }

    /// The projects a list may show: a form-selector grant adds none (fails closed).
    #[must_use]
    pub fn projects(&self) -> ProjectFilter {
        let mut ids = Vec::new();
        for (project, form) in &self.grants {
            if form.is_some() {
                continue;
            }
            match (*project, self.pinned) {
                (None, None) => return ProjectFilter::All,
                (None, Some(only)) | (Some(only), None) => ids.push(only),
                (Some(project), Some(pinned)) if project == pinned => ids.push(project),
                (Some(_), Some(_)) => {}
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ProjectFilter::Only(ids)
    }
}

tokio::task_local! {
    static COVER: PluginCover;
}

/// The cover of the plugin call the current request belongs to, if any.
#[must_use]
pub fn current_cover() -> Option<PluginCover> {
    COVER.try_with(Clone::clone).ok()
}

/// Whether a caller counts as automated for the human-only gates (the bot-schema rule, the
/// published-site draft rules): a bot, or any plugin call, which acts for a person but is code.
#[must_use]
pub fn automated(is_bot: bool) -> bool {
    is_bot || current().is_some()
}

/// The `source` a handler stores for a write by `actor`.
///
/// It is the plugin's provenance when the request is a plugin call (`{type: "plugin", surface:
/// "plugin", installation, generation, actor}`), else the one of a person or a bot.
///
/// The host strips any `source` a guest sends, so this is the only way a plugin source reaches a
/// stored record or event (contract C1 §3, MF-4).
#[must_use]
pub fn provenance_source(is_bot: bool, actor_id: Uuid) -> serde_json::Value {
    current().map_or_else(
        || serde_json::json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        |call| {
            serde_json::json!({
                "type": "plugin",
                "surface": crate::flow::event_origin::EventSurface::Plugin.as_wire(),
                "installation": call.installation_id,
                "generation": call.generation,
                "actor": call.actor_id,
            })
        },
    )
}

/// Runs `future` as a request of the plugin call `call`.
pub async fn scope<F: std::future::Future>(call: PluginCall, future: F) -> F::Output {
    CURRENT.scope(call, future).await
}

/// The request extension the host puts on a request it dispatches. Extensions are set by server
/// code only and never arrive from the wire, so a request from outside never carries it.
#[derive(Clone, Debug)]
pub struct InternalCall {
    /// The operation the host checked; the guard refuses a route whose entry has another `op`.
    pub op: &'static str,
    /// The principal as the host built it; the authentication branch re-checks it.
    pub principal: Arc<Principal>,
    /// The invoking user.
    pub actor: Uuid,
    /// `users.session_version` of the actor when the invocation started (contract §3, N-2).
    pub session_version: i64,
    pub installation_id: Uuid,
    pub generation: i32,
    /// 1 for a call from a guest; a call made while handling one is refused.
    pub depth: u8,
    pub request_id: Uuid,
}

impl InternalCall {
    #[must_use]
    pub const fn marker(&self) -> PluginCall {
        PluginCall {
            installation_id: self.installation_id,
            generation: self.generation,
            actor_kind: "user",
            actor_id: self.actor,
        }
    }
}

/// The claims of an internal call: the invoking user, live.
///
/// A user who is gone or deactivated, or whose sessions were revoked since the invocation started
/// (logout-all, password reset), or who is not a person (a bot or AI user), is refused `403
/// forbidden` and the invocation ends.
///
/// # Errors
/// `Forbidden` as above; a database failure.
pub async fn internal_claims(state: &AppState, call: &InternalCall) -> Result<JwtClaims, ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT email, is_active, session_version, entity_type FROM users WHERE id = $1",
            vec![call.actor.into()],
        ))
        .await
        .map_err(ApiError::internal_database_failure)?;
    let forbidden = || ApiError::Forbidden("forbidden".to_string());
    let row = row.ok_or_else(forbidden)?;
    let email: String = row.try_get("", "email").map_err(|_| ApiError::Internal)?;
    let active: bool = row.try_get("", "is_active").map_err(|_| ApiError::Internal)?;
    let version: i64 = row.try_get("", "session_version").map_err(|_| ApiError::Internal)?;
    let entity_type: String = row.try_get("", "entity_type").map_err(|_| ApiError::Internal)?;
    // Only a person acts through a plugin: a bot or AI user would run with plain user claims and
    // skip the bot context's project scoping.
    if !active || entity_type != "human" || version != call.session_version {
        return Err(forbidden());
    }
    Ok(JwtClaims {
        sub: call.actor.to_string(),
        email,
        token_type: TokenType::Access,
        iat: 0,
        exp: 0,
        sv: version,
    })
}

/// The authentication branch of an internal call, shared by the two authentication middlewares: no
/// token is parsed.
///
/// The call is checked once more against the entry of the matched route and the resolved target,
/// the invoking user is loaded live, and the handler runs with that user's claims and the plugin
/// marker ([`current`]).
///
/// # Errors
/// `plugin_reentry` for a depth other than 1, a catalogue refusal when the principal is not
/// allowed, `Forbidden` when the user is gone or the session changed.
pub async fn handle_internal(
    state: &AppState,
    request: Request,
    next: Next,
    call: InternalCall,
) -> Result<Response, ApiError> {
    if call.depth != 1 {
        return Err(Refusal::new(RefusalCode::PluginReentry, None).into());
    }
    let (mut parts, body) = request.into_parts();
    let not_exposed = || ApiError::from(Refusal::new(RefusalCode::CapabilityNotExposed, None));
    let entry = parts
        .extensions
        .get::<MatchedPath>()
        .and_then(|matched| by_route(parts.method.as_str(), matched.as_str()))
        .filter(|entry| entry.op == call.op)
        .ok_or_else(not_exposed)?;
    allowed_pre(&call.principal, entry)?;
    let raw = RawPathParams::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| not_exposed())?;
    let params: Vec<(String, String)> = raw
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let resource = resource::resolve_resource(&state.db, entry, &params).await?;
    let Some(resource) = resource else {
        return Err(Refusal::new(RefusalCode::CapabilityScopeMismatch, Some(entry.capability())).into());
    };
    allowed_target(&call.principal, entry, &resource)?;
    let claims = internal_claims(state, &call).await?;
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(claims);
    let (Principal::Plugin { installation, .. } | Principal::Installation { installation, .. }) =
        call.principal.as_ref()
    else {
        return Err(not_exposed());
    };
    let cover = PluginCover::new(installation, entry.capability());
    Ok(COVER.scope(cover, scope(call.marker(), next.run(request))).await)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{PluginCall, provenance_source, scope};

    #[tokio::test]
    async fn a_write_names_the_plugin_only_inside_a_plugin_call() {
        let actor = Uuid::from_u128(7);
        assert_eq!(
            provenance_source(false, actor),
            json!({"type": "user", "actor_id": actor})
        );
        assert_eq!(
            provenance_source(true, actor),
            json!({"type": "bot", "actor_id": actor})
        );
        let call = PluginCall {
            installation_id: Uuid::from_u128(1),
            generation: 3,
            actor_kind: "user",
            actor_id: actor,
        };
        let inside = scope(call, async { provenance_source(false, Uuid::from_u128(99)) }).await;
        assert_eq!(
            inside,
            json!({
                "type": "plugin", "surface": "plugin",
                "installation": Uuid::from_u128(1), "generation": 3, "actor": actor,
            })
        );
    }
}
