//! Which tmux session a task of a project and CLI is typed into: a pure function of the runner's
//! configuration (and, in a later stage, a name the server sends).
//!
//! 1. A local entry (`[[runner.tmux.sessions]]`) for the project key and CLI names an existing
//!    session: [`NameSource::Local`].
//! 2. A name from the server is accepted only when the operator allows it. Stage A reads no server
//!    name and has no allow list, so a server name is always refused.
//! 3. Else the derived `<KEY>-<cli>`: [`NameSource::Derived`], the only kind the runner creates.
//!
//! The slot id (`<KEY>-<cli>`, a [`SessionName`]) never changes with the tmux name: it keys the
//! worker, the state directory and the hook routing.

use super::checks::Refusal;
use super::config::RunnerConfig;
use super::registry::SessionRegistry;
use super::tmux::{NameSource, SessionName, TmuxName};

/// The answer of [`resolve`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The slot: worker key, state directory, hook routing.
    pub slot: SessionName,
    /// The tmux session tasks are typed into.
    pub tmux: TmuxName,
    pub source: NameSource,
}

/// Resolves the tmux session of `key` and `cli`. `server` is the name the server sent, if any.
///
/// # Errors
///
/// A refusal for an invalid key or CLI, an invalid configured name, or a server name the
/// operator did not allow.
pub fn resolve(cfg: &RunnerConfig, key: &str, cli: &str, server: Option<&str>) -> Result<Resolved, Refusal> {
    let slot = SessionName::new(key, cli)
        .map_err(|_| Refusal::new("invalid_project_key", "the project key is not a valid session name"))?;
    if let Some(entry) = cfg.session_override(key, cli) {
        let tmux = TmuxName::new(&entry.name).map_err(|_| {
            Refusal::new(
                "session_config_invalid",
                "the runner's configured session name is not valid",
            )
        })?;
        if server.is_some_and(|name| !name.eq_ignore_ascii_case(&entry.name)) {
            tracing::info!(
                session = slot.as_str(),
                "the runner's own session name wins over the server's"
            );
        }
        return Ok(Resolved {
            slot,
            tmux,
            source: NameSource::Local,
        });
    }
    if server.is_some() {
        return Err(Refusal::new(
            "session_name_not_allowed",
            "this runner does not accept a session name from Sylvode",
        ));
    }
    Ok(Resolved {
        tmux: TmuxName::derived(&slot),
        slot,
        source: NameSource::Derived,
    })
}

/// A local entry is keyed by project key. When the registry says the project of a delivery owns
/// another key that has local entries, the project was renamed and the entries are stale: the
/// runner would otherwise start a fresh derived session next to the operator's one.
///
/// # Errors
///
/// `session_config_stale`.
pub fn check_stale(cfg: &RunnerConfig, registry: &SessionRegistry, project_id: &str, key: &str) -> Result<(), Refusal> {
    let stale = cfg
        .tmux
        .sessions
        .iter()
        .find(|entry| entry.project != key && registry.owner(&entry.project).as_deref() == Some(project_id));
    stale.map_or(Ok(()), |entry| {
        Err(Refusal::new(
            "session_config_stale",
            format!(
                "the project key changed from {} to {key}; update runner.tmux.sessions on this runner",
                entry.project
            ),
        ))
    })
}
