#![allow(clippy::indexing_slicing, clippy::literal_string_with_formatting_args)]

use std::path::Path;

use super::config::{RunnerClaudeConfig, RunnerCodexConfig};
use super::launch::{
    CodexLaunchFiles, LaunchFiles, SEARCH_MODE_TOOLS, claude_headless_argv, claude_hook_command, codex_headless_argv,
    codex_session_argv, hook_words, mcp_tool, opencode_headless_argv, session_argv, shell_quote, toml_array,
    toml_string,
};

fn files() -> LaunchFiles<'static> {
    LaunchFiles {
        claude_mcp_json: Path::new("/state/mcp/BOT.claude.json"),
        claude_settings: Path::new("/state/sessions/SVWL-claude/claude-settings.json"),
        runner_exe: Path::new("/opt/sylvode/bin/sylvode-runner"),
        hook_socket: Path::new("/state/hook.sock"),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// The argument vector of a session the runner creates: the words of `launch_command`, then the
/// runner's MCP file and (with hooks) its settings. Nothing restricts the CLI: no tool list, no
/// strict MCP config, no setting-source limit, no permission mode of the runner's own.
#[test]
fn a_created_session_runs_the_launch_command_plus_the_mcp_file_and_the_hook_settings() {
    let program = Path::new("/usr/local/bin/claude");
    let argv = session_argv(program, &RunnerClaudeConfig::default(), &files()).expect("argv");
    assert_eq!(
        argv,
        strings(&[
            "/usr/local/bin/claude",
            "--permission-mode",
            "bypassPermissions",
            "--mcp-config",
            "/state/mcp/BOT.claude.json",
            "--settings",
            "/state/sessions/SVWL-claude/claude-settings.json",
        ])
    );
    for refused_flag in [
        "--strict-mcp-config",
        "--setting-sources",
        "--allowedTools",
        "--disallowedTools",
    ] {
        assert!(!argv.iter().any(|a| a == refused_flag), "{refused_flag}");
    }
    // The operator's own words are used as they are; `hooks = false` leaves out `--settings`.
    let custom = RunnerClaudeConfig {
        launch_command: strings(&["claude", "--model", "opus", "--verbose"]),
        hooks: false,
        ..RunnerClaudeConfig::default()
    };
    assert_eq!(
        session_argv(program, &custom, &files()).expect("argv"),
        strings(&[
            "/usr/local/bin/claude",
            "--model",
            "opus",
            "--verbose",
            "--mcp-config",
            "/state/mcp/BOT.claude.json",
        ])
    );
    // The program is the resolved path of the first word, not the word.
    assert_eq!(
        session_argv(Path::new("/x/y"), &custom, &files()).expect("argv")[0],
        "/x/y"
    );
    // An empty command cannot be launched.
    let empty = RunnerClaudeConfig {
        launch_command: Vec::new(),
        ..RunnerClaudeConfig::default()
    };
    assert!(session_argv(program, &empty, &files()).is_err());
}

/// The placeholder and append matrix: a word that is exactly a placeholder is replaced; what the
/// template does not place itself is appended; hooks off leaves out `--settings` unless the
/// template placed it.
#[test]
fn the_launch_command_placeholders_and_the_append_matrix() {
    let program = Path::new("/bin/claude");
    let run = |words: &[&str], hooks: bool| {
        let cfg = RunnerClaudeConfig {
            launch_command: strings(words),
            hooks,
            ..RunnerClaudeConfig::default()
        };
        session_argv(program, &cfg, &files()).expect("argv")
    };
    let mcp = "/state/mcp/BOT.claude.json";
    let settings = "/state/sessions/SVWL-claude/claude-settings.json";
    // Neither placeholder: both are appended (hooks on), or only the MCP file (hooks off).
    assert_eq!(
        run(&["claude", "-x"], true),
        strings(&["/bin/claude", "-x", "--mcp-config", mcp, "--settings", settings])
    );
    assert_eq!(
        run(&["claude", "-x"], false),
        strings(&["/bin/claude", "-x", "--mcp-config", mcp])
    );
    // `{mcp_config}` placed: the template owns its position, only `--settings` is appended.
    assert_eq!(
        run(&["claude", "--mcp-config", "{mcp_config}", "-x"], true),
        strings(&["/bin/claude", "--mcp-config", mcp, "-x", "--settings", settings])
    );
    // `{settings}` placed: only `--mcp-config` is appended.
    assert_eq!(
        run(&["claude", "--settings", "{settings}", "-x"], true),
        strings(&["/bin/claude", "--settings", settings, "-x", "--mcp-config", mcp])
    );
    // Both placed: nothing is appended, in any order, hooks on or off.
    for hooks in [true, false] {
        assert_eq!(
            run(
                &["claude", "--settings", "{settings}", "--mcp-config", "{mcp_config}"],
                hooks
            ),
            strings(&["/bin/claude", "--settings", settings, "--mcp-config", mcp])
        );
    }
    // `{settings}` placed with hooks off is still replaced (the template owns it).
    assert_eq!(
        run(&["claude", "--settings", "{settings}"], false),
        strings(&["/bin/claude", "--settings", settings, "--mcp-config", mcp])
    );
    // Whole words only: a word that merely contains a placeholder is not a placement, stays
    // literal, and the file is appended as if it were absent.
    assert_eq!(
        run(&["claude", "--mcp-config={mcp_config}"], false),
        strings(&["/bin/claude", "--mcp-config={mcp_config}", "--mcp-config", mcp])
    );
    // The program word is never substituted, and a placeholder is replaced everywhere it stands
    // as a whole word.
    assert_eq!(
        run(&["claude", "{mcp_config}", "{mcp_config}"], false),
        strings(&["/bin/claude", mcp, mcp])
    );
}

#[test]
fn the_headless_claude_argv_is_the_prompt_first_then_the_operators_arguments() {
    let mcp = Path::new("/state/mcp/BOT.claude.json");
    let argv = claude_headless_argv(
        Path::new("/usr/local/bin/claude"),
        &RunnerClaudeConfig::default(),
        mcp,
        false,
        "[Sylvode task] id=x",
    )
    .expect("argv");
    let mut expected = strings(&[
        "/usr/local/bin/claude",
        "-p",
        "[Sylvode task] id=x",
        "--permission-mode",
        "bypassPermissions",
        "--mcp-config",
        "/state/mcp/BOT.claude.json",
        "--allowedTools",
    ]);
    expected.extend(SEARCH_MODE_TOOLS.iter().map(|tool| mcp_tool(tool)));
    assert_eq!(argv, expected);
    assert_eq!(SEARCH_MODE_TOOLS.len(), 8);
    // The write tool is added, never replaces the list.
    let wide = claude_headless_argv(Path::new("/c"), &RunnerClaudeConfig::default(), mcp, true, "p").expect("argv");
    assert_eq!(wide.last().map(String::as_str), Some("mcp__sylvode__sylvode_write"));
    assert!(wide.contains(&"mcp__sylvode__sylvode_read".to_string()));
    // The operator's arguments replace the default ones.
    let custom = RunnerClaudeConfig {
        headless_args: strings(&["--model", "x"]),
        ..RunnerClaudeConfig::default()
    };
    let argv = claude_headless_argv(Path::new("/c"), &custom, mcp, false, "p").expect("argv");
    assert_eq!(argv[3..5], ["--model", "x"]);
    assert!(!argv.iter().any(|a| a == "bypassPermissions"));
    // A prompt that would be read as an option is refused.
    for bad in ["", "-x", "--help"] {
        assert!(
            claude_headless_argv(Path::new("/c"), &RunnerClaudeConfig::default(), mcp, false, bad).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn the_headless_codex_and_opencode_argv() {
    let codex = codex_headless_argv(
        Path::new("/usr/bin/codex"),
        &RunnerCodexConfig::default(),
        Path::new("/opt/mcp-server"),
        Path::new("/state/mcp/BOT.toml"),
        "[Sylvode task] id=x",
    )
    .expect("argv");
    assert_eq!(
        codex[..3],
        strings(&["/usr/bin/codex", "exec", "--skip-git-repo-check"])
    );
    assert_eq!(codex.last().map(String::as_str), Some("[Sylvode task] id=x"));
    assert!(
        !codex.iter().any(|a| a == "--sandbox"),
        "the operator chooses the sandbox"
    );
    let with_args = codex_headless_argv(
        Path::new("/usr/bin/codex"),
        &RunnerCodexConfig {
            headless_args: strings(&["--sandbox", "workspace-write"]),
            ..RunnerCodexConfig::default()
        },
        Path::new("/opt/mcp-server"),
        Path::new("/state/mcp/BOT.toml"),
        "p",
    )
    .expect("argv");
    let n = with_args.len();
    assert_eq!(with_args[n - 3..], strings(&["--sandbox", "workspace-write", "p"]));
    assert!(with_args.iter().any(|a| a.starts_with("mcp_servers.sylvode.command=")));
    assert!(
        codex_headless_argv(
            Path::new("/c"),
            &RunnerCodexConfig::default(),
            Path::new("/m"),
            Path::new("/t"),
            "-x"
        )
        .is_err()
    );
    assert_eq!(
        opencode_headless_argv(Path::new("/usr/bin/opencode"), "p").expect("argv"),
        strings(&["/usr/bin/opencode", "run", "p"])
    );
    assert!(opencode_headless_argv(Path::new("/o"), "-x").is_err());
}

#[test]
fn toml_values_survive_hostile_text() {
    for text in ["plain", "a\"b", "a\\b", "a\nb", "a'b", "tab\there", "ünï", "=[]#"] {
        let table: toml::Table = toml::from_str(&format!("v = {}", toml_string(text))).expect("parses");
        assert_eq!(table["v"].as_str(), Some(text));
    }
    let items = strings(&["a\"b", "c\nd"]);
    let table: toml::Table = toml::from_str(&format!("v = {}", toml_array(&items))).expect("parses");
    assert_eq!(table["v"].as_array().map(Vec::len), Some(2));
}

#[test]
fn hook_commands_are_fixed_words_and_quoted_values() {
    let words = hook_words(&files(), "claude", "Stop").expect("words");
    assert_eq!(
        words,
        strings(&[
            "/opt/sylvode/bin/sylvode-runner",
            "hook",
            "--cli",
            "claude",
            "--event",
            "Stop",
            "--socket",
            "/state/hook.sock",
        ])
    );
    assert_eq!(
        claude_hook_command(&files(), "Stop").expect("command"),
        "'/opt/sylvode/bin/sylvode-runner' 'hook' '--cli' 'claude' '--event' 'Stop' '--socket' '/state/hook.sock'"
    );
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote("a b; rm -rf /"), "'a b; rm -rf /'");
    // A path with a control character is refused, not quoted.
    let weird = LaunchFiles {
        runner_exe: Path::new("/opt/run\nner"),
        ..files()
    };
    assert!(hook_words(&weird, "claude", "Stop").is_err());
    assert!(claude_hook_command(&weird, "Stop").is_err());
}

/// What a shell makes of the hook command is exactly its words, whatever the paths hold.
#[test]
fn a_quoted_hook_command_is_parsed_by_a_shell_into_the_same_words() {
    let exe = "/tmp/dir with 'quote' and $HOME and `id`/sylvode-runner";
    let socket = "/tmp/state \"x\"/hook.sock";
    let weird = LaunchFiles {
        runner_exe: Path::new(exe),
        hook_socket: Path::new(socket),
        ..files()
    };
    let command = claude_hook_command(&weird, "Stop").expect("command");
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("printf '%s\\n' {command}"))
        .output()
        .expect("sh");
    let words: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(words, hook_words(&weird, "claude", "Stop").expect("words"));
}

fn codex_files() -> CodexLaunchFiles<'static> {
    CodexLaunchFiles {
        mcp_server: Path::new("/opt/sylvode/bin/mcp-server"),
        mcp_toml: Path::new("/state/mcp/BOT.toml"),
        runner_exe: Path::new("/opt/sylvode/bin/sylvode-runner"),
        hook_socket: Path::new("/state/hook.sock"),
    }
}

const CODEX_MCP_COMMAND: &str = "mcp_servers.sylvode.command=\"/opt/sylvode/bin/mcp-server\"";
const CODEX_MCP_ARGS: &str =
    "mcp_servers.sylvode.args=[\"serve\", \"--transport\", \"stdio\", \"--config\", \"/state/mcp/BOT.toml\"]";
const CODEX_NOTIFY: &str = "notify=[\"/opt/sylvode/bin/sylvode-runner\", \"hook\", \"--cli\", \"codex\", \
                            \"--event\", \"turn-ended\", \"--socket\", \"/state/hook.sock\"]";

/// A codex session: the launch words, then the MCP server and the turn hook as `-c` overrides.
#[test]
fn a_created_codex_session_gets_the_mcp_server_and_the_notify_hook_as_overrides() {
    let program = Path::new("/usr/local/bin/codex");
    let argv = codex_session_argv(program, &RunnerCodexConfig::default(), &codex_files()).expect("argv");
    assert_eq!(
        argv,
        strings(&[
            "/usr/local/bin/codex",
            "--no-daemon",
            "--no-alt-screen",
            "--dangerously-bypass-approvals-and-sandbox",
            "-c",
            CODEX_MCP_COMMAND,
            "-c",
            CODEX_MCP_ARGS,
            "-c",
            CODEX_NOTIFY,
        ])
    );
    // `hooks = false` leaves out the notify override only.
    let no_hooks = RunnerCodexConfig {
        hooks: false,
        ..RunnerCodexConfig::default()
    };
    let argv = codex_session_argv(program, &no_hooks, &codex_files()).expect("argv");
    assert_eq!(argv.len(), 8);
    assert!(!argv.iter().any(|word| word.starts_with("notify=")));
}

/// A placeholder is replaced by its value (the template writes the `-c`) and owns its position;
/// the others are still appended. Whole words only.
#[test]
fn the_codex_placeholders_own_their_position() {
    let program = Path::new("/usr/local/bin/codex");
    let cfg = RunnerCodexConfig {
        launch_command: strings(&[
            "codex",
            "-c",
            "{notify_override}",
            "--model",
            "x",
            "-c",
            "{mcp_args_override}",
        ]),
        ..RunnerCodexConfig::default()
    };
    let argv = codex_session_argv(program, &cfg, &codex_files()).expect("argv");
    assert_eq!(
        argv,
        strings(&[
            "/usr/local/bin/codex",
            "-c",
            CODEX_NOTIFY,
            "--model",
            "x",
            "-c",
            CODEX_MCP_ARGS,
            "-c",
            CODEX_MCP_COMMAND,
        ])
    );
    let all = RunnerCodexConfig {
        launch_command: strings(&[
            "codex",
            "-c",
            "{mcp_command_override}",
            "-c",
            "{mcp_args_override}",
            "-c",
            "{notify_override}",
        ]),
        hooks: false,
        ..RunnerCodexConfig::default()
    };
    // A placed notify override is written even with hooks off, like claude's `{settings}`.
    let argv = codex_session_argv(program, &all, &codex_files()).expect("argv");
    assert_eq!(
        argv[1..],
        strings(&["-c", CODEX_MCP_COMMAND, "-c", CODEX_MCP_ARGS, "-c", CODEX_NOTIFY])
    );
    // A word that merely contains a placeholder is left alone, and the value is still appended.
    let partial = RunnerCodexConfig {
        launch_command: strings(&["codex", "x{notify_override}"]),
        ..RunnerCodexConfig::default()
    };
    let argv = codex_session_argv(program, &partial, &codex_files()).expect("argv");
    assert_eq!(argv[1], "x{notify_override}");
    assert_eq!(argv.len(), 8);
}

/// No vector holds a token (it is in the file the vector names), and unusable input is refused.
#[test]
fn a_codex_session_argv_holds_no_token_and_refuses_unusable_input() {
    let program = Path::new("/usr/local/bin/codex");
    let argv = codex_session_argv(program, &RunnerCodexConfig::default(), &codex_files()).expect("argv");
    assert!(!argv.join(" ").to_lowercase().contains("token"));
    let empty = RunnerCodexConfig {
        launch_command: Vec::new(),
        ..RunnerCodexConfig::default()
    };
    assert!(codex_session_argv(program, &empty, &codex_files()).is_err());
    let hostile = CodexLaunchFiles {
        hook_socket: Path::new("/state/hook\n.sock"),
        ..codex_files()
    };
    assert!(codex_session_argv(program, &RunnerCodexConfig::default(), &hostile).is_err());
    // The same override values as the headless run.
    let headless = codex_headless_argv(
        program,
        &RunnerCodexConfig::default(),
        Path::new("/opt/sylvode/bin/mcp-server"),
        Path::new("/state/mcp/BOT.toml"),
        "go",
    )
    .expect("headless");
    assert!(headless.iter().any(|w| w == CODEX_MCP_COMMAND) && headless.iter().any(|w| w == CODEX_MCP_ARGS));
}
