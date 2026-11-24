use super::checks::Channel;
use super::config::RunnerConfig;
use super::naming::{resolve, tmux_of_slot};
use super::tmux::{NameSource, Slot};

const BOT: &str = "8c1f0a52-3d4e-4b6a-9c7d-1e2f3a4b5c6d";
const OTHER_BOT: &str = "9d2a1b63-4e5f-4c7b-8d8e-2f3a4b5c6d7e";

fn config(sessions: &str) -> RunnerConfig {
    toml::from_str(&format!("[tmux]\nsocket_name = \"\"\n{sessions}")).expect("runner config")
}

fn entry(bot: &str, cli: &str, name: &str) -> String {
    format!("[[tmux.sessions]]\nbot = \"{bot}\"\ncli = \"{cli}\"\nname = \"{name}\"\n")
}

#[test]
fn with_no_entry_every_session_is_the_derived_one() {
    let cfg = config("");
    for (key, cli) in [("SV", "claude"), ("SV", "codex"), ("RF9", "claude")] {
        let found = resolve(&cfg, BOT, key, cli, Channel::Tmux).expect("resolves");
        assert_eq!(found.source, NameSource::Derived);
        assert!(!found.slot.is_bot());
        assert_eq!(found.slot.as_str(), format!("{key}-{cli}"));
        assert_eq!(found.tmux.as_str(), found.slot.as_str());
        assert_eq!(found.tmux.session_target(), format!("={key}-{cli}"));
    }
}

/// An entry of a bot wins for every project, whatever the project key is.
#[test]
fn a_bot_entry_serves_every_project_of_the_bot() {
    let cfg = config(&entry(BOT, "claude", "sv"));
    for key in ["SV", "RFGUI", "PRC"] {
        let found = resolve(&cfg, BOT, key, "claude", Channel::Tmux).expect("resolves");
        assert_eq!(found.source, NameSource::Local);
        assert_eq!(found.tmux.as_str(), "sv");
        assert_eq!(found.slot.as_str(), format!("bot-{BOT}-claude"));
        assert_eq!(found.slot.bot_id(), Some(BOT));
    }
    // The bot id compares whatever its case.
    let upper = resolve(&cfg, &BOT.to_uppercase(), "SV", "claude", Channel::Tmux).expect("resolves");
    assert_eq!(upper.source, NameSource::Local);
}

/// A headless delivery never uses the bot's session: the channel setting means what it says.
#[test]
fn a_headless_delivery_ignores_the_bot_entry() {
    let cfg = config(&entry(BOT, "claude", "sv"));
    let found = resolve(&cfg, BOT, "SV", "claude", Channel::Headless).expect("resolves");
    assert_eq!(found.source, NameSource::Derived);
    assert!(!found.slot.is_bot());
    assert_eq!(found.slot.as_str(), "SV-claude");
    let tmux = resolve(&cfg, BOT, "SV", "claude", Channel::Tmux).expect("resolves");
    assert_eq!(tmux.source, NameSource::Local);
}

/// Another CLI of the bot, and another bot, keep the derived session of the project.
#[test]
fn another_cli_or_bot_falls_back_to_the_derived_session() {
    let cfg = config(&entry(BOT, "claude", "sv"));
    for (bot, key, cli) in [(BOT, "SV", "codex"), (OTHER_BOT, "SV", "claude")] {
        let other = resolve(&cfg, bot, key, cli, Channel::Tmux).expect("resolves");
        assert_eq!(
            (other.source, other.tmux.as_str().to_string()),
            (NameSource::Derived, format!("{key}-{cli}"))
        );
        assert_eq!(other.slot.key(), Some(key));
    }
}

#[test]
fn an_invalid_key_or_cli_is_refused() {
    let cfg = config("");
    assert_eq!(
        resolve(&cfg, BOT, "sv", "claude", Channel::Tmux).expect_err("key").code,
        "invalid_project_key"
    );
    assert_eq!(
        resolve(&cfg, BOT, "SV", "vim", Channel::Tmux).expect_err("cli").code,
        "invalid_project_key"
    );
}

#[test]
fn a_slot_falls_back_to_its_configured_or_derived_name() {
    let cfg = config(&entry(BOT, "claude", "sv"));
    let bot_slot = Slot::bot(BOT, "claude").expect("slot");
    let (tmux, source) = tmux_of_slot(&cfg, &bot_slot).expect("entry");
    assert_eq!((tmux.as_str(), source), ("sv", NameSource::Local));
    // The entry is gone: nothing names the session.
    assert!(tmux_of_slot(&config(""), &bot_slot).is_none());
    let project = Slot::project("SV", "codex").expect("slot");
    let (tmux, source) = tmux_of_slot(&cfg, &project).expect("derived");
    assert_eq!((tmux.as_str(), source), ("SV-codex", NameSource::Derived));
}
