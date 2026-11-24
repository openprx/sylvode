#![allow(clippy::indexing_slicing)]

use std::time::{Duration, SystemTime};

use sylvode_prereq::Status;

use super::config::RunnerConfig;
use super::files::{CreatedBy, SessionRecord};
use super::session_report::{
    Assessment, Mcp, McpEntry, Observation, Permissions, ProcRow, SessionState, assess, cli_process, codex_permissions,
    parse_etime, parse_process_rows, permissions_for, postures_of,
};
use super::tmux::{ListedPane, NameSource, PaneId, Slot, TmuxName};

/// What the CLI's process arguments look like: the default launch command, an operator's own
/// start, and a start with the older flag.
const ARGS_BYPASS: &str =
    "claude --permission-mode bypassPermissions --mcp-config /s/mcp/B.claude.json --settings /s/x.json";
const ARGS_BYPASS_EQUALS: &str = "/usr/local/bin/claude --permission-mode=bypassPermissions";
const ARGS_SKIP: &str = "claude --dangerously-skip-permissions";
const ARGS_NORMAL: &str = "claude --model opus --permission-mode acceptEdits";
/// The footer claude draws in the input chrome while permission checks are off.
const FOOTER_BYPASS: &str = "> \n  \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle)";
const FOOTER_PLAIN: &str = "> \n  ? for shortcuts";

#[test]
fn bypass_is_read_from_the_arguments_first_and_the_footer_second() {
    for args in [ARGS_BYPASS, ARGS_BYPASS_EQUALS, ARGS_SKIP] {
        assert_eq!(
            permissions_for("claude", Some(args), None),
            Permissions::Bypass,
            "{args}"
        );
        assert_eq!(
            permissions_for("claude", Some(args), Some(FOOTER_PLAIN)),
            Permissions::Bypass,
            "{args}"
        );
    }
    assert_eq!(
        permissions_for("claude", Some(ARGS_NORMAL), Some(FOOTER_PLAIN)),
        Permissions::Normal
    );
    assert_eq!(permissions_for("claude", Some(ARGS_NORMAL), None), Permissions::Normal);
    // The footer catches a CLI whose mode was switched after it started.
    assert_eq!(
        permissions_for("claude", Some(ARGS_NORMAL), Some(FOOTER_BYPASS)),
        Permissions::Bypass
    );
    assert_eq!(
        permissions_for("claude", None, Some(FOOTER_BYPASS)),
        Permissions::Bypass
    );
    // Nothing known: unknown, never "normal".
    assert_eq!(
        permissions_for("claude", None, Some(FOOTER_PLAIN)),
        Permissions::Unknown
    );
    assert_eq!(permissions_for("claude", None, None), Permissions::Unknown);
    // A word that merely mentions the mode is not the flag.
    assert_eq!(
        permissions_for("claude", Some("claude --append-system-prompt bypassPermissions"), None),
        Permissions::Normal
    );
}

fn pane(id: &str, pid: u32, command: &str, dead: bool) -> ListedPane {
    ListedPane {
        id: PaneId::new(id).expect("pane id"),
        pid,
        dead,
        command: command.to_string(),
    }
}

fn record(created_by: CreatedBy) -> SessionRecord {
    SessionRecord {
        name: "QLA-claude".to_string(),
        project_id: "p".to_string(),
        bot_id: "bot".to_string(),
        cli: "claude".to_string(),
        pane_id: "%1".to_string(),
        pane_pid: 100,
        dir: "/x".to_string(),
        created_by,
        pushed: None,
        tmux_name: "QLA-claude".to_string(),
        name_source: "derived".to_string(),
    }
}

/// A running operator session with everything in place, to be spoiled one field at a time.
fn healthy() -> Observation {
    Observation {
        name: Slot::project("QLA", "claude").expect("name"),
        tmux: TmuxName::derived(&Slot::project("QLA", "claude").expect("name")).expect("derived"),
        source: NameSource::Derived,
        exists: true,
        panes: vec![pane("%0", 99, "zsh", false), pane("%1", 100, "claude", false)],
        process_names: vec!["claude".to_string(), "claude.exe".to_string()],
        record: Some(record(CreatedBy::Operator)),
        screen: Some(FOOTER_PLAIN.to_string()),
        cli_args: Some(ARGS_NORMAL.to_string()),
        mcp_in_args: true,
        mcp_entry: None,
        credentials_newer: Some(false),
        hooks_seen: Some(SystemTime::now()),
        foreground: None,
        pane_id: None,
    }
}

fn row(obs: &Observation) -> Assessment {
    assess(obs)
}

#[test]
fn a_session_that_does_not_exist_is_absent_and_ok() {
    let mut obs = healthy();
    obs.exists = false;
    obs.panes.clear();
    let absent = row(&obs);
    assert_eq!((absent.state, absent.status), (SessionState::Absent, Status::Ok));
    // What the record says about who started it is kept; nothing else is claimed.
    assert_eq!(absent.created_by, Some(CreatedBy::Operator));
    assert_eq!((absent.hooks, absent.permissions, absent.mcp), (None, None, None));
    obs.record = None;
    assert_eq!(row(&obs).created_by, None);
}

#[test]
fn a_healthy_session_has_every_field_and_is_ok() {
    let ok = row(&healthy());
    assert_eq!(ok.state, SessionState::Running);
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.created_by, Some(CreatedBy::Operator));
    assert_eq!(ok.hooks, Some(true));
    assert_eq!(ok.permissions, Some(Permissions::Normal));
    assert_eq!(ok.mcp, Some(Mcp::Ok));
    assert_eq!(ok.pane_id.as_deref(), Some("%1"));
    assert_eq!(ok.foreground.as_deref(), Some("claude"));
    assert_eq!(ok.bot_id, "bot");
    // A session the runner started is told so, by the record, when the pane is still the recorded one.
    let mut by_runner = healthy();
    by_runner.record = Some(record(CreatedBy::Runner));
    assert_eq!(row(&by_runner).created_by, Some(CreatedBy::Runner));
}

#[test]
fn no_cli_pane_and_two_cli_panes_fail() {
    let mut shell = healthy();
    shell.panes = vec![pane("%0", 99, "zsh", false)];
    let fails = row(&shell);
    assert_eq!((fails.state, fails.status), (SessionState::CliNotRunning, Status::Fail));
    assert_eq!((fails.hooks, fails.permissions, fails.mcp), (None, None, None));
    let mut dead = healthy();
    dead.panes = vec![pane("%1", 100, "claude", true)];
    assert_eq!(row(&dead).state, SessionState::CliNotRunning);
    let mut two = healthy();
    two.panes.push(pane("%2", 101, "claude", false));
    let ambiguous = row(&two);
    assert_eq!(
        (ambiguous.state, ambiguous.status),
        (SessionState::Ambiguous, Status::Fail)
    );
}

#[test]
fn bypass_no_hooks_stale_and_a_dialog_warn_and_a_missing_mcp_fails() {
    let mut bypass = healthy();
    bypass.cli_args = Some(ARGS_BYPASS.to_string());
    let warn = row(&bypass);
    assert_eq!(
        (warn.permissions, warn.status),
        (Some(Permissions::Bypass), Status::Warn)
    );

    let mut quiet = healthy();
    quiet.hooks_seen = None;
    let warn = row(&quiet);
    assert_eq!((warn.hooks, warn.status), (Some(false), Status::Warn));

    let mut stale = healthy();
    stale.credentials_newer = Some(true);
    let warn = row(&stale);
    assert_eq!((warn.mcp, warn.status), (Some(Mcp::Stale), Status::Warn));

    let mut dialog = healthy();
    dialog.screen = Some("Is this a project you created or one you trust?\n  Yes, I trust this folder".to_string());
    let warn = row(&dialog);
    assert_eq!((warn.state, warn.status), (SessionState::Dialog, Status::Warn));

    // The MCP server: missing and another one fail; found by `claude mcp get` is fine.
    for (entry, want, status) in [
        (McpEntry::Absent, Mcp::Missing, Status::Fail),
        (McpEntry::Elsewhere, Mcp::Other, Status::Fail),
        (McpEntry::Ours, Mcp::Ok, Status::Ok),
    ] {
        let mut by_entry = healthy();
        by_entry.mcp_in_args = false;
        by_entry.mcp_entry = Some(entry);
        let found = row(&by_entry);
        assert_eq!((found.mcp, found.status), (Some(want), status), "{entry:?}");
    }
    // Nothing known about the MCP server: no claim, no failure.
    let mut unknown = healthy();
    unknown.mcp_in_args = false;
    assert_eq!((row(&unknown).mcp, row(&unknown).status), (None, Status::Ok));
    // Stale only softens an entry that works; a missing one stays a failure.
    let mut missing_and_newer = healthy();
    missing_and_newer.mcp_in_args = false;
    missing_and_newer.mcp_entry = Some(McpEntry::Absent);
    missing_and_newer.credentials_newer = Some(true);
    assert_eq!(row(&missing_and_newer).mcp, Some(Mcp::Missing));
    // A failure outranks a warning.
    let mut both = missing_and_newer;
    both.cli_args = Some(ARGS_BYPASS.to_string());
    assert_eq!(row(&both).status, Status::Fail);
}

#[test]
fn elapsed_times_and_the_process_table_parse() {
    for (text, secs) in [
        ("00:07", 7),
        ("12:34", 754),
        ("1:02:03", 3723),
        ("2-03:04:05", 2 * 86_400 + 3 * 3600 + 4 * 60 + 5),
        ("01:00", 60),
    ] {
        assert_eq!(parse_etime(text), Some(Duration::from_secs(secs)), "{text}");
    }
    for bad in ["", "abc", "1:2:3:4", "1-", "x:10"] {
        assert_eq!(parse_etime(bad), None, "{bad:?}");
    }
    let table = parse_process_rows(
        "  100     1 01:00 -zsh\n  200   100 00:30 claude --permission-mode bypassPermissions\n  300   200 00:05 node tool\n\
         garbage line\n  400 xx 00:01 broken\n",
    );
    assert_eq!(table.len(), 3);
    assert_eq!(
        table[1],
        ProcRow {
            pid: 200,
            ppid: 100,
            elapsed: Duration::from_secs(30),
            args: "claude --permission-mode bypassPermissions".to_string()
        }
    );
    // The CLI is found among the pane process and its descendants, by its program name.
    let names = vec!["claude".to_string()];
    let found = cli_process(&table, 100, &names).expect("claude below the shell");
    assert_eq!(found.pid, 200);
    // Started directly in the pane (a session the runner created): the pane process itself.
    assert_eq!(cli_process(&table, 200, &names).map(|r| r.pid), Some(200));
    // Not below the pane, or not the CLI: nothing.
    assert!(cli_process(&table, 300, &names).is_none());
    assert!(cli_process(&table, 100, &["codex".to_string()]).is_none());
    // A program given by path still matches by its file name.
    let by_path = parse_process_rows("10 1 00:01 /opt/bin/claude --x\n");
    assert_eq!(cli_process(&by_path, 10, &names).map(|r| r.pid), Some(10));
}

fn posture_row(rows: &[(&'static str, super::session_report::Posture)], cli: &str) -> Option<(bool, bool)> {
    rows.iter()
        .find(|(name, _)| *name == cli)
        .map(|(_, posture)| (posture.tmux, posture.headless))
}

#[test]
fn the_posture_says_whether_each_channel_turns_permission_checks_off() {
    // The defaults: claude bypasses on both channels; codex is off and has no row.
    let defaults = postures_of(&RunnerConfig::default());
    assert_eq!(posture_row(&defaults, "claude"), Some((true, true)));
    assert_eq!(posture_row(&defaults, "codex"), None);
    assert_eq!(posture_row(&defaults, "opencode"), None);
    let narrowed = toml::from_str::<RunnerConfig>(
        "[cli.claude]\nlaunch_command = [\"claude\", \"--model\", \"x\"]\nheadless_args = []\n",
    )
    .expect("runner config");
    assert_eq!(posture_row(&postures_of(&narrowed), "claude"), Some((false, false)));
    let only_headless = toml::from_str::<RunnerConfig>(
        "[cli.claude]\nlaunch_command = [\"claude\"]\nheadless_args = [\"--dangerously-skip-permissions\"]\n",
    )
    .expect("runner config");
    assert_eq!(posture_row(&postures_of(&only_headless), "claude"), Some((false, true)));
    // codex has its own row once enabled: the default launch command bypasses on tmux, the
    // headless arguments decide the headless channel.
    let codex = toml::from_str::<RunnerConfig>(
        "[cli.claude]\nlaunch_command = [\"claude\"]\nheadless_args = []\n\
         [cli.codex]\nenabled = true\nheadless_args = [\"--dangerously-bypass-approvals-and-sandbox\"]\n",
    )
    .expect("runner config");
    assert_eq!(posture_row(&postures_of(&codex), "codex"), Some((true, true)));
    let sandboxed = toml::from_str::<RunnerConfig>(
        "[cli.claude]\nlaunch_command = [\"claude\"]\nheadless_args = []\n\
         [cli.codex]\nenabled = true\nlaunch_command = [\"codex\", \"-a\", \"never\", \"-s\", \"workspace-write\"]\n",
    )
    .expect("runner config");
    assert_eq!(posture_row(&postures_of(&sandboxed), "codex"), Some((false, false)));
    let off = toml::from_str::<RunnerConfig>(
        "[cli.claude]\nlaunch_command = [\"claude\"]\nheadless_args = []\n\
         [cli.codex]\nheadless_args = [\"--dangerously-bypass-approvals-and-sandbox\"]\n",
    )
    .expect("runner config");
    assert_eq!(posture_row(&postures_of(&off), "codex"), None);
    // opencode's permissions are not controlled: headless counts as bypass while it is enabled.
    let opencode = toml::from_str::<RunnerConfig>("[cli.opencode]\nenabled = true\n").expect("runner config");
    assert_eq!(posture_row(&postures_of(&opencode), "opencode"), Some((false, true)));
}

/// codex's permissions from its launch words (design table), in every spelling.
#[test]
fn codex_permissions_are_read_from_its_words() {
    let words = |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_string).collect() };
    for (text, want) in [
        ("codex --dangerously-bypass-approvals-and-sandbox", Permissions::Bypass),
        ("codex -a never -s danger-full-access", Permissions::Bypass),
        (
            "codex --ask-for-approval never --sandbox danger-full-access",
            Permissions::Bypass,
        ),
        (
            "codex --ask-for-approval=never --sandbox=danger-full-access",
            Permissions::Bypass,
        ),
        (
            "codex -c approval_policy=never -c sandbox_mode=danger-full-access",
            Permissions::Bypass,
        ),
        (
            "codex -c approval_policy=\"never\" -c sandbox_mode=\"danger-full-access\"",
            Permissions::Bypass,
        ),
        ("codex -a never -s workspace-write", Permissions::Normal),
        ("codex --sandbox=read-only", Permissions::Normal),
        ("codex -a on-request", Permissions::Normal),
        ("codex -a on-request -s danger-full-access", Permissions::Normal),
        ("codex --no-daemon --no-alt-screen", Permissions::Unknown),
        ("codex -a never", Permissions::Unknown),
        ("codex -s danger-full-access", Permissions::Unknown),
        ("codex -a", Permissions::Unknown),
        ("codex --model never", Permissions::Unknown),
    ] {
        assert_eq!(codex_permissions(&words(text)), want, "{text}");
    }
    // The banner of the bypass mode is the secondary sign when the words say nothing.
    let banner = "permissions: YOLO mode\n\u{203a} Ask Codex to do anything";
    assert_eq!(
        permissions_for("codex", Some("codex"), Some(banner)),
        Permissions::Bypass
    );
    assert_eq!(permissions_for("codex", None, Some(banner)), Permissions::Bypass);
    assert_eq!(
        permissions_for("codex", Some("codex -a never -s workspace-write"), None),
        Permissions::Normal
    );
    assert_eq!(permissions_for("codex", None, None), Permissions::Unknown);
    // claude's footer never marks a codex pane and the other way round.
    assert_eq!(
        permissions_for("codex", None, Some(FOOTER_BYPASS)),
        Permissions::Unknown
    );
    assert_eq!(permissions_for("claude", None, Some(banner)), Permissions::Unknown);
}

/// A session with a configured name is looked at under that name; an absent one is a warning
/// (its tasks fail until the operator starts it), a derived absent one is fine.
#[test]
fn an_absent_configured_session_is_a_warning() {
    let mut obs = healthy();
    obs.exists = false;
    obs.panes.clear();
    obs.tmux = TmuxName::new("sv").expect("name");
    obs.source = NameSource::Local;
    let absent = row(&obs);
    assert_eq!((absent.state, absent.status), (SessionState::Absent, Status::Warn));
    assert_eq!((absent.tmux.as_str(), absent.source), ("sv", NameSource::Local));
    obs.source = NameSource::Derived;
    assert_eq!(row(&obs).status, Status::Ok);
}

/// A foreign `sylvode` entry fails a session the runner may start, and only warns for a configured
/// one (the operator's own session holds its own entry, which must hold this bot's token).
#[test]
fn a_foreign_mcp_entry_is_a_warning_for_a_configured_session() {
    let mut obs = healthy();
    obs.mcp_in_args = false;
    obs.mcp_entry = Some(McpEntry::Elsewhere);
    let derived = row(&obs);
    assert_eq!((derived.mcp, derived.status), (Some(Mcp::Other), Status::Fail));
    obs.tmux = TmuxName::new("sv").expect("name");
    obs.source = NameSource::Local;
    let named = row(&obs);
    assert_eq!((named.mcp, named.status), (Some(Mcp::Other), Status::Warn));
    // A missing entry still fails a configured session: the CLI cannot reach Sylvode at all.
    obs.mcp_entry = Some(McpEntry::Absent);
    assert_eq!(row(&obs).status, Status::Fail);
}

/// A bot slot is its bot's whether or not a record exists, so the report sends its row to that
/// bot only; a project slot takes the bot from its record.
#[test]
fn a_bot_slot_names_its_bot_without_a_record() {
    const BOT: &str = "8c1f0a52-3d4e-4b6a-9c7d-1e2f3a4b5c6d";
    let mut obs = healthy();
    obs.name = Slot::bot(BOT, "claude").expect("bot slot");
    obs.tmux = TmuxName::new("sv").expect("tmux name");
    obs.source = NameSource::Local;
    obs.exists = false;
    obs.record = None;
    let row = assess(&obs);
    assert_eq!(row.bot_id, BOT);
    // Never created, so an absent session is a warning.
    assert_eq!(row.status, Status::Warn);
    // A project slot has no bot of its own: the record's.
    let mut project = healthy();
    project.exists = false;
    assert_eq!(assess(&project).bot_id, record(CreatedBy::Operator).bot_id);
    project.record = None;
    assert_eq!(assess(&project).bot_id, "");
}
