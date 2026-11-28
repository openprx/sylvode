//! Which slot and which tmux session a task of a bot, project and CLI goes to: a pure function of
//! the runner's configuration.
//!
//! 1. For a tmux delivery, a local entry (`[[runner.tmux.sessions]]`) for the bot and CLI names
//!    existing sessions that serve the bot for every project: [`Slot::Bot`] and
//!    [`NameSource::Local`]. The first name is member 0, the primary, which is what [`resolve`]
//!    returns; the session pool picks the member that takes a task. The runner never creates such
//!    a session. A headless delivery never uses it: the channel setting means what it says.
//! 2. Else the derived `<KEY>-<cli>`: [`Slot::Project`] and [`NameSource::Derived`], the only kind
//!    the runner creates.
//!
//! The slot keys the worker, the state directory and the hook routing; the tmux name is only the
//! target of the driver.

use super::checks::{Channel, Refusal};
use super::config::RunnerConfig;
use super::tmux::{NameSource, Slot, TmuxName};

/// The answer of [`resolve`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The slot: worker key, state directory, hook routing.
    pub slot: Slot,
    /// The tmux session tasks are typed into.
    pub tmux: TmuxName,
    pub source: NameSource,
}

/// Resolves the slot and tmux session of a task for `bot` in project `key` for `cli` on `channel`.
///
/// # Errors
///
/// A refusal for an invalid key or CLI, or an invalid configured name.
pub fn resolve(cfg: &RunnerConfig, bot: &str, key: &str, cli: &str, channel: Channel) -> Result<Resolved, Refusal> {
    if channel == Channel::Tmux
        && let Some(entry) = cfg.bot_session(bot, cli)
    {
        let invalid = || Refusal::new("session_config_invalid", "the runner's configured session is not valid");
        return Ok(Resolved {
            slot: Slot::bot(&entry.bot, cli).map_err(|_| invalid())?,
            tmux: TmuxName::new(entry.primary().ok_or_else(invalid)?).map_err(|_| invalid())?,
            source: NameSource::Local,
        });
    }
    let invalid = || Refusal::new("invalid_project_key", "the project key is not a valid session name");
    let slot = Slot::project(key, cli).map_err(|_| invalid())?;
    Ok(Resolved {
        tmux: TmuxName::derived(&slot).ok_or_else(invalid)?,
        slot,
        source: NameSource::Derived,
    })
}

/// The tmux session a slot is served by when nothing else says: the derived name of a project
/// slot, the configured name of a bot slot's member (`None` when its entry or the member is gone).
pub fn tmux_of_slot(cfg: &RunnerConfig, slot: &Slot) -> Option<(TmuxName, NameSource)> {
    match slot.bot_id() {
        Some(bot) => {
            let entry = cfg.bot_session(bot, slot.cli())?;
            Some((TmuxName::new(entry.name_of(slot.member())?).ok()?, NameSource::Local))
        }
        None => Some((TmuxName::derived(slot)?, NameSource::Derived)),
    }
}
