use super::config::RunnerConfig;
use super::naming::{check_stale, resolve};
use super::registry::SessionRegistry;
use super::tmux::NameSource;

const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "33333333-3333-4333-8333-333333333333";

fn config(sessions: &str) -> RunnerConfig {
    toml::from_str(&format!("[tmux]\nsocket_name = \"\"\n{sessions}")).expect("runner config")
}

const ENTRY: &str = "[[tmux.sessions]]\nproject = \"SV\"\ncli = \"claude\"\nname = \"sv\"\n";

#[test]
fn with_no_entry_every_session_is_the_derived_one() {
    let cfg = config("");
    for (key, cli) in [("SV", "claude"), ("SV", "codex"), ("RF9", "claude")] {
        let found = resolve(&cfg, key, cli, None).expect("resolves");
        assert_eq!(found.source, NameSource::Derived);
        assert_eq!(found.slot.as_str(), format!("{key}-{cli}"));
        assert_eq!(found.tmux.as_str(), found.slot.as_str());
        assert_eq!(found.tmux.session_target(), format!("={key}-{cli}"));
    }
}

#[test]
fn a_local_entry_names_the_tmux_session_and_leaves_the_slot_alone() {
    let cfg = config(ENTRY);
    let found = resolve(&cfg, "SV", "claude", None).expect("resolves");
    assert_eq!(found.source, NameSource::Local);
    assert_eq!(found.tmux.as_str(), "sv");
    assert_eq!(found.slot.as_str(), "SV-claude");
    // Another CLI or another project keeps its derived name.
    for (key, cli) in [("SV", "codex"), ("OTHER", "claude")] {
        let other = resolve(&cfg, key, cli, None).expect("resolves");
        assert_eq!(
            (other.source, other.tmux.as_str().to_string()),
            (NameSource::Derived, format!("{key}-{cli}"))
        );
    }
}

/// The entry wins over whatever the server sends; with no entry a server name is never accepted
/// in this stage.
#[test]
fn the_local_entry_wins_and_a_server_name_is_not_accepted_yet() {
    let cfg = config(ENTRY);
    let found = resolve(&cfg, "SV", "claude", Some("other")).expect("resolves");
    assert_eq!((found.source, found.tmux.as_str()), (NameSource::Local, "sv"));
    let refusal = resolve(&config(""), "SV", "claude", Some("sv")).expect_err("refused");
    assert_eq!(refusal.code, "session_name_not_allowed");
}

#[test]
fn an_invalid_key_or_cli_is_refused() {
    let cfg = config("");
    assert_eq!(
        resolve(&cfg, "sv", "claude", None).expect_err("key").code,
        "invalid_project_key"
    );
    assert_eq!(
        resolve(&cfg, "SV", "vim", None).expect_err("cli").code,
        "invalid_project_key"
    );
}

#[test]
fn a_renamed_project_makes_the_entries_of_its_old_key_stale() {
    let registry = SessionRegistry::open(None).expect("registry");
    let cfg = config(ENTRY);
    // Nothing is bound: not stale.
    assert!(check_stale(&cfg, &registry, PROJECT, "SV").is_ok());
    registry.bind(PROJECT, "SV").expect("bind");
    // The project still uses its key.
    assert!(check_stale(&cfg, &registry, PROJECT, "SV").is_ok());
    // Another project is not affected by the first one's entry.
    assert!(check_stale(&cfg, &registry, OTHER, "RF").is_ok());
    // The project came back under another key.
    let refusal = check_stale(&cfg, &registry, PROJECT, "SVX").expect_err("stale");
    assert_eq!(refusal.code, "session_config_stale");
    assert!(
        refusal.reason.contains("SV") && refusal.reason.contains("SVX"),
        "{}",
        refusal.reason
    );
    // With no entry for the old key nothing is stale.
    assert!(check_stale(&config(""), &registry, PROJECT, "SVX").is_ok());
}
