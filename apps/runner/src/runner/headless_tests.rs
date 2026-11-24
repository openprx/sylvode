#![allow(clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::checks::{Accepted, BotCreds, Channel};
use super::files::StateDir;
use super::headless::{Prepared, execute, prepare};
use super::sv_client::SvClient;
use super::tmux::{NameSource, Slot, TmuxName, agent_environment_from};
use crate::config::Config;

const BOT: &str = "00000000-0000-4000-8000-000000000001";

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-headless-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        Self(dir.canonicalize().expect("canonical"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config(extra: &str) -> Config {
    Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
         [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:2\"\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{}\"\nsurface = \"mcp_stdio\"\n\
         [runner]\nallowed_roots = [\"/\"]\n{extra}\n",
        "k9".repeat(14)
    ))
    .expect("config")
}

fn accepted(cli: &str, dir: &Path, key: &str, program: &Path) -> Accepted {
    Accepted {
        client: SvClient::new(reqwest::Client::new(), "http://127.0.0.1:2", "opr_x", "mcp_stdio"),
        bot: BotCreds {
            bot_id: BOT.to_string(),
            api_url: "http://127.0.0.1:2".to_string(),
            workspace_id: "11111111-1111-4111-8111-111111111111".to_string(),
            token: format!("opr_{}", "k9".repeat(14)),
        },
        task_id: "11111111-1111-4111-8111-111111111111".to_string(),
        attempt: 1,
        max_attempts: 3,
        project_key: key.to_string(),
        slot: Slot::project(key, cli).expect("slot"),
        tmux: TmuxName::derived(&Slot::project(key, cli).expect("slot")).expect("derived"),
        name_source: NameSource::Derived,
        project_id: "22222222-2222-4222-8222-222222222222".to_string(),
        message: "[Sylvode task] id=x".to_string(),
        dir: dir.to_path_buf(),
        dir_checked: true,
        channel: Channel::Headless,
        cli: cli.to_string(),
        program: program.to_path_buf(),
        mcp_server: PathBuf::from("/opt/mcp-server"),
    }
}

/// A headless claude run: `-p <prompt>` first, the operator's `headless_args`, the runner's MCP
/// file, then the allow list of the eight Sylvode tools. No setting-source, strict-config or
/// permission flag of the runner's own.
#[test]
fn the_claude_argv_is_the_prompt_the_operators_arguments_the_mcp_file_and_the_tool_list() {
    let base = Dir::new();
    let state = StateDir::open(&base.0.join("state")).expect("state");
    let project = base.0.join("project");
    fs::create_dir_all(&project).expect("mkdir");
    let accepted = accepted("claude", &project, "SVWL", Path::new("/usr/bin/claude"));
    let prepared = prepare(&config(""), &state, &accepted, Path::new("/opt/mcp-server")).expect("prepared");
    let json = state.claude_mcp_json(BOT).to_string_lossy().into_owned();
    let mut expected: Vec<String> = [
        "/usr/bin/claude",
        "-p",
        "[Sylvode task] id=x",
        "--permission-mode",
        "bypassPermissions",
        "--mcp-config",
        &json,
        "--allowedTools",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    expected.extend(
        super::launch::SEARCH_MODE_TOOLS
            .iter()
            .map(|t| format!("mcp__sylvode__{t}")),
    );
    assert_eq!(prepared.args, expected);
    assert!(
        !prepared
            .args
            .iter()
            .any(|a| a.starts_with("--strict") || a == "--setting-sources")
    );
    // The operator's arguments replace the default; the write call is added by `allow_write_call`.
    let custom =
        config("[runner.cli.claude]\nheadless_args = [\"--model\", \"x\"]\n[runner.mcp]\nallow_write_call = true\n");
    let prepared = prepare(&custom, &state, &accepted, Path::new("/opt/mcp-server")).expect("prepared");
    assert_eq!(&prepared.args[3..5], ["--model", "x"]);
    assert!(!prepared.args.iter().any(|a| a == "bypassPermissions"));
    assert_eq!(
        prepared.args.last().map(String::as_str),
        Some("mcp__sylvode__sylvode_write")
    );
    let toml = fs::read_to_string(state.mcp_toml(BOT)).expect("toml");
    assert!(toml.contains("allow_write_call = true"), "{toml}");
}

/// codex: `exec`, the MCP server through `-c`, the operator's `headless_args` and the prompt last;
/// opencode: `run <prompt>`.
#[test]
fn the_codex_and_opencode_argvs() {
    let base = Dir::new();
    let state = StateDir::open(&base.0.join("state")).expect("state");
    let project = base.0.join("project");
    fs::create_dir_all(&project).expect("mkdir");
    // A project that holds codex configuration is no longer refused: the operator's choice.
    fs::create_dir_all(project.join(".codex")).expect("mkdir");
    let cfg = config("[runner.cli.codex]\nenabled = true\nheadless_args = [\"--sandbox\", \"read-only\"]\n");
    let prepared = prepare(
        &cfg,
        &state,
        &accepted("codex", &project, "SVWL", Path::new("/usr/bin/codex")),
        Path::new("/opt/mcp-server"),
    )
    .expect("codex");
    assert_eq!(&prepared.args[..3], ["/usr/bin/codex", "exec", "--skip-git-repo-check"]);
    let n = prepared.args.len();
    assert_eq!(
        &prepared.args[n - 3..],
        ["--sandbox", "read-only", "[Sylvode task] id=x"]
    );
    assert!(
        prepared
            .args
            .iter()
            .any(|a| a.starts_with("mcp_servers.sylvode.command="))
    );
    let plain = prepare(
        &config(""),
        &state,
        &accepted("codex", &project, "SVWL", Path::new("/usr/bin/codex")),
        Path::new("/opt/mcp-server"),
    )
    .expect("codex");
    assert_eq!(plain.args.last().map(String::as_str), Some("[Sylvode task] id=x"));
    assert!(!plain.args.iter().any(|a| a == "--sandbox"));
    fs::write(project.join("opencode.json"), "{}").expect("write");
    let opencode = prepare(
        &config(""),
        &state,
        &accepted("opencode", &project, "SVWL", Path::new("/usr/bin/opencode")),
        Path::new("/m"),
    )
    .expect("opencode");
    assert_eq!(opencode.args, ["/usr/bin/opencode", "run", "[Sylvode task] id=x"]);
}

#[test]
fn prepare_writes_private_files_and_an_argv_without_the_token() {
    let base = Dir::new();
    let state = StateDir::open(&base.0.join("state")).expect("state");
    let project = base.0.join("project");
    fs::create_dir_all(&project).expect("mkdir");
    let cfg = config("");
    let prepared = prepare(
        &cfg,
        &state,
        &accepted("claude", &project, "SVWL", Path::new("/usr/bin/claude")),
        Path::new("/opt/mcp-server"),
    )
    .expect("prepared");
    assert!(prepared.args.iter().all(|a| !a.contains("opr_")), "{:?}", prepared.args);
    let toml = state.mcp_toml(BOT);
    assert_eq!(fs::metadata(&toml).expect("meta").permissions().mode() & 0o777, 0o600);
    assert!(fs::read_to_string(&toml).expect("toml").contains("opr_"));
    assert!(
        !fs::read_to_string(state.claude_mcp_json(BOT))
            .expect("json")
            .contains("opr_")
    );
    // The runner forces no configuration directory of its own.
    assert!(!state.root().join("claude").exists());
}

/// The CLI gets the operator's environment: everything but the three agent variables, nothing
/// added (no `CLAUDE_CONFIG_DIR` of the runner's).
#[test]
fn the_environment_is_the_operators_minus_the_three_agent_variables() {
    let vars = |names: &[&str]| -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        names.iter().map(|n| ((*n).into(), "v".into())).collect()
    };
    let kept: Vec<String> = agent_environment_from(vars(&[
        "PATH",
        "HOME",
        "ANTHROPIC_API_KEY",
        "HTTPS_PROXY",
        "CLAUDE_CONFIG_DIR",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "TMUX",
        "TMUX_PANE",
    ]))
    .into_iter()
    .map(|(n, _)| n.to_string_lossy().into_owned())
    .collect();
    assert_eq!(
        kept,
        [
            "PATH",
            "HOME",
            "ANTHROPIC_API_KEY",
            "HTTPS_PROXY",
            "CLAUDE_CONFIG_DIR",
            "TMUX_PANE"
        ]
    );
}

/// A run really starts with that environment: a stand-in program (not a CLI agent) writes what it
/// sees. The test process runs under cargo, so cargo's variables are the operator's here.
#[tokio::test]
async fn a_headless_run_has_the_operators_variables_and_none_of_the_agent_ones() {
    let base = Dir::new();
    let out = base.0.join("env.txt");
    let script = base.0.join("standin.sh");
    fs::write(&script, "#!/bin/sh\nenv > \"$1\"\n").expect("script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    let prepared = Prepared {
        program: script.clone(),
        args: vec![
            script.to_string_lossy().into_owned(),
            out.to_string_lossy().into_owned(),
        ],
    };
    let end = execute(
        &prepared,
        &base.0,
        std::time::Duration::from_secs(20),
        std::future::pending(),
    )
    .await;
    assert_eq!(end.status, "success");
    let text = fs::read_to_string(&out).expect("env");
    let names: Vec<&str> = text.lines().filter_map(|l| l.split_once('=')).map(|(n, _)| n).collect();
    assert!(
        names.contains(&"PATH") && names.contains(&"CARGO_MANIFEST_DIR"),
        "{names:?}"
    );
    for gone in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"] {
        assert!(!names.contains(&gone), "{gone}");
    }
    // Nothing is added: the child has CLAUDE_CONFIG_DIR exactly when the runner's own environment
    // has it.
    assert_eq!(
        names.contains(&"CLAUDE_CONFIG_DIR"),
        std::env::var_os("CLAUDE_CONFIG_DIR").is_some()
    );
}

#[test]
fn the_start_up_posture_has_one_line_per_cli_and_warns_about_bypass_and_opencode() {
    let lines = super::posture(&config(""));
    assert_eq!(lines.len(), 3);
    // The defaults turn permission checks off: a warning, naming both commands.
    assert!(
        lines[0].warn && lines[0].text.contains("PERMISSION CHECKS ARE OFF") && lines[0].text.contains("claude"),
        "{:?}",
        lines[0]
    );
    assert!(lines[1].text.starts_with("codex: disabled") && !lines[1].warn);
    assert_eq!(lines[2].text, "opencode: disabled");
    let normal = super::posture(&config(
        "[runner.cli.claude]\nlaunch_command = [\"claude\", \"--model\", \"x\"]\nheadless_args = []\n\
         [runner.cli.opencode]\nenabled = true\n",
    ));
    assert!(!normal[0].warn && !normal[0].text.contains("OFF"), "{:?}", normal[0]);
    assert!(
        normal[2].warn
            && normal[2]
                .text
                .contains("opencode permissions are not controlled by sylvode-runner")
    );
}

#[test]
fn bypass_is_recognised_in_command_words() {
    let words = |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_string).collect() };
    for yes in [
        "claude --dangerously-skip-permissions",
        "claude --permission-mode bypassPermissions",
        "claude --permission-mode=bypassPermissions --model x",
    ] {
        assert!(super::words_bypass_permissions(&words(yes)), "{yes}");
    }
    for no in [
        "claude",
        "claude --permission-mode acceptEdits",
        "claude --permission-mode",
        "claude --model bypassPermissions",
    ] {
        assert!(!super::words_bypass_permissions(&words(no)), "{no}");
    }
}
