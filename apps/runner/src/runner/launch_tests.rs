#![allow(clippy::indexing_slicing)]

use std::path::Path;

use super::config::{RunnerClaudeConfig, RunnerCodexConfig};
use super::launch::{
    LaunchError, LaunchFiles, SEARCH_MODE_TOOLS, claude_argv, claude_hook_command, codex_argv, hook_words, mcp_tool,
    opencode_argv, rule_safe_dir, shell_quote, toml_array, toml_string,
};
use super::tmux::SessionKey;

fn files() -> LaunchFiles<'static> {
    LaunchFiles {
        mcp_server: Path::new("/opt/sylvode/bin/mcp-server"),
        mcp_toml: Path::new("/state/mcp/BOT.toml"),
        claude_mcp_json: Path::new("/state/mcp/BOT.claude.json"),
        claude_settings: Path::new("/state/sessions/SVWL/claude-settings.json"),
        runner_exe: Path::new("/opt/sylvode/bin/sylvode-runner"),
        hook_socket: Path::new("/state/hook.sock"),
    }
}

fn key() -> SessionKey {
    SessionKey::new("SVWL").expect("key")
}

const DIR: &str = "/work/svwl";

fn claude(cfg: &RunnerClaudeConfig) -> Result<Vec<String>, LaunchError> {
    claude_argv(Path::new("/usr/local/bin/claude"), cfg, Path::new(DIR), &files())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn claude_defaults_are_exactly_the_documented_posture() {
    let argv = claude(&RunnerClaudeConfig::default()).expect("argv");
    assert_eq!(
        argv,
        strings(&[
            "/usr/local/bin/claude",
            "--permission-mode",
            "dontAsk",
            "--mcp-config",
            "/state/mcp/BOT.claude.json",
            "--strict-mcp-config",
            "--settings",
            "/state/sessions/SVWL/claude-settings.json",
            "--allowedTools",
            "mcp__sylvode__sylvode_overview",
            "mcp__sylvode__sylvode_search",
            "mcp__sylvode__sylvode_read",
            "mcp__sylvode__sylvode_task_get",
            "mcp__sylvode__sylvode_task_ack",
            "mcp__sylvode__sylvode_task_progress",
            "mcp__sylvode__sylvode_task_complete",
            "mcp__sylvode__sylvode_task_fail",
            "Read(//work/svwl/**)",
            "Glob(//work/svwl/**)",
            "Grep(//work/svwl/**)",
        ])
    );
    // Neither the write tool nor any shell or edit tool, and no skip-permissions flag.
    let text = argv.join(" ");
    for absent in [
        "sylvode_write",
        "Bash",
        "Edit",
        "Write",
        "dangerously",
        "bypass",
        "-p ",
        "--print",
    ] {
        assert!(!text.contains(absent), "{absent}: {text}");
    }
    assert_eq!(SEARCH_MODE_TOOLS.len(), 8);
    assert_eq!(mcp_tool("sylvode_read"), "mcp__sylvode__sylvode_read");
}

#[test]
fn each_widening_adds_only_what_it_names() {
    let base = claude(&RunnerClaudeConfig::default()).expect("argv");
    let write = claude(&RunnerClaudeConfig {
        allow_write_call: true,
        ..RunnerClaudeConfig::default()
    })
    .expect("argv");
    assert_eq!(write.len(), base.len() + 1);
    assert!(write.contains(&"mcp__sylvode__sylvode_write".to_string()));

    let extras = claude(&RunnerClaudeConfig {
        allowed_tools: strings(&["Edit(/{dir}/src/**)", "Bash(git status)"]),
        disallowed_tools: strings(&["Bash(rm *)"]),
        ..RunnerClaudeConfig::default()
    })
    .expect("argv");
    assert!(extras.contains(&"Edit(//work/svwl/src/**)".to_string()));
    assert!(extras.contains(&"Bash(git status)".to_string()));
    let tail = &extras[extras.len() - 2..];
    assert_eq!(tail, ["--disallowedTools", "Bash(rm *)"]);
    // The mode is the configured one, validated.
    let accept = claude(&RunnerClaudeConfig {
        permission_mode: "acceptEdits".to_string(),
        ..RunnerClaudeConfig::default()
    })
    .expect("argv");
    assert_eq!(accept[2], "acceptEdits");
}

#[test]
fn bypassing_permissions_needs_the_acknowledgement_and_nothing_else_skips_them() {
    let bypass = RunnerClaudeConfig {
        permission_mode: "bypassPermissions".to_string(),
        ..RunnerClaudeConfig::default()
    };
    assert!(matches!(claude(&bypass), Err(LaunchError::Permissions(_))));
    let acknowledged = RunnerClaudeConfig {
        dangerously_bypass_permissions: true,
        ..bypass
    };
    assert_eq!(claude(&acknowledged).expect("argv")[2], "bypassPermissions");
    // The flag alone, a wrong mode, an empty or option-like entry.
    for bad in [
        RunnerClaudeConfig {
            dangerously_bypass_permissions: true,
            ..RunnerClaudeConfig::default()
        },
        RunnerClaudeConfig {
            permission_mode: "yolo".to_string(),
            ..RunnerClaudeConfig::default()
        },
        RunnerClaudeConfig {
            allowed_tools: strings(&["--dangerously-skip-permissions"]),
            ..RunnerClaudeConfig::default()
        },
        RunnerClaudeConfig {
            allowed_tools: strings(&[""]),
            ..RunnerClaudeConfig::default()
        },
        RunnerClaudeConfig {
            disallowed_tools: strings(&["Bash\nx"]),
            ..RunnerClaudeConfig::default()
        },
    ] {
        assert!(matches!(claude(&bad), Err(LaunchError::Permissions(_))), "{bad:?}");
    }
}

#[test]
fn the_directory_in_rules_is_the_canonical_path_and_only_plain_characters() {
    for good in [
        "/work/svwl",
        "/work/my project",
        "/Users/a.b/work-1/x_y+z@host",
        "/work/Überprüfung",
    ] {
        assert_eq!(rule_safe_dir(Path::new(good)), Ok(good));
    }
    for bad in [
        "relative",
        "",
        "/work/a(b)",
        "/work/a*",
        "/work/a?",
        "/work/a[1]",
        "/work/a{b}",
        "/work/a,b",
        "/work/a\"b",
        "/work/a'b",
        "/work/a#b",
        "/work/a\nb",
        "/work/a\\b",
        "/work/a;b",
        "/work/a`b",
        "/work/a$b",
        "/work/a|b",
    ] {
        assert!(
            matches!(rule_safe_dir(Path::new(bad)), Err(LaunchError::Unsafe(_))),
            "{bad:?}"
        );
        assert!(
            claude_argv(
                Path::new("/c"),
                &RunnerClaudeConfig::default(),
                Path::new(bad),
                &files()
            )
            .is_err()
        );
    }
    let error = rule_safe_dir(Path::new("/work/a(b)")).expect_err("refused").to_string();
    assert!(!error.contains("/work"), "{error}");
}

#[test]
fn codex_defaults_and_values_round_trip_through_a_toml_parser() {
    let argv = codex_argv(
        Path::new("/usr/local/bin/codex"),
        &RunnerCodexConfig::default(),
        Path::new(DIR),
        &files(),
        &key(),
    )
    .expect("argv");
    assert_eq!(
        &argv[..7],
        ["/usr/local/bin/codex", "-s", "read-only", "-a", "never", "-C", DIR]
    );
    assert_eq!(argv.iter().filter(|a| *a == "-c").count(), 3);
    let values: Vec<&String> = argv.iter().skip(7).filter(|a| *a != "-c").collect();
    assert_eq!(values.len(), 3);
    // Each `-c` value is `key=value` with the value in TOML; parse it back.
    let parse = |entry: &str| -> toml::Value {
        let (key, value) = entry.split_once('=').expect("key=value");
        let table: toml::Table = toml::from_str(&format!("v = {value}")).expect("valid toml");
        let _ = key;
        table["v"].clone()
    };
    assert_eq!(values[0].split_once('=').expect("kv").0, "mcp_servers.sylvode.command");
    assert_eq!(
        parse(values[0]),
        toml::Value::String("/opt/sylvode/bin/mcp-server".to_string())
    );
    assert_eq!(values[1].split_once('=').expect("kv").0, "mcp_servers.sylvode.args");
    assert_eq!(
        parse(values[1]),
        toml::Value::Array(
            ["serve", "--transport", "stdio", "--config", "/state/mcp/BOT.toml"]
                .iter()
                .map(|s| toml::Value::String((*s).to_string()))
                .collect()
        )
    );
    assert_eq!(values[2].split_once('=').expect("kv").0, "notify");
    assert_eq!(
        parse(values[2]),
        toml::Value::Array(
            [
                "/opt/sylvode/bin/sylvode-runner",
                "hook",
                "--cli",
                "codex",
                "--session",
                "SVWL",
                "--event",
                "turn-ended",
                "--socket",
                "/state/hook.sock"
            ]
            .iter()
            .map(|s| toml::Value::String((*s).to_string()))
            .collect()
        )
    );
}

#[test]
fn codex_sandbox_widening_needs_the_acknowledgement_and_the_bypass_flag_is_never_used() {
    let run = |cfg: RunnerCodexConfig| codex_argv(Path::new("/c"), &cfg, Path::new(DIR), &files(), &key());
    assert_eq!(
        run(RunnerCodexConfig {
            sandbox: "workspace-write".to_string(),
            ..RunnerCodexConfig::default()
        })
        .expect("argv")[2],
        "workspace-write"
    );
    assert!(matches!(
        run(RunnerCodexConfig {
            sandbox: "danger-full-access".to_string(),
            ..RunnerCodexConfig::default()
        }),
        Err(LaunchError::Permissions(_))
    ));
    let full = run(RunnerCodexConfig {
        sandbox: "danger-full-access".to_string(),
        dangerously_bypass_permissions: true,
        ..RunnerCodexConfig::default()
    })
    .expect("argv");
    assert_eq!(full[2], "danger-full-access");
    for argv in [&full, &run(RunnerCodexConfig::default()).expect("argv")] {
        assert!(argv.iter().all(|a| !a.contains("dangerously-bypass")), "{argv:?}");
        assert_eq!(argv[3..5], ["-a", "never"]);
    }
    // The flag with a narrower sandbox contradicts it.
    assert!(
        run(RunnerCodexConfig {
            dangerously_bypass_permissions: true,
            ..RunnerCodexConfig::default()
        })
        .is_err()
    );
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
fn opencode_gets_no_permission_arguments() {
    assert_eq!(
        opencode_argv(Path::new("/usr/bin/opencode")),
        Ok(strings(&["/usr/bin/opencode"]))
    );
}

#[test]
fn hook_commands_are_fixed_words_and_quoted_values() {
    let words = hook_words(&files(), "claude", &key(), "Stop").expect("words");
    assert_eq!(
        words,
        strings(&[
            "/opt/sylvode/bin/sylvode-runner",
            "hook",
            "--cli",
            "claude",
            "--session",
            "SVWL",
            "--event",
            "Stop",
            "--socket",
            "/state/hook.sock"
        ])
    );
    assert_eq!(
        claude_hook_command(&files(), &key(), "Stop").expect("command"),
        "'/opt/sylvode/bin/sylvode-runner' 'hook' '--cli' 'claude' '--session' 'SVWL' '--event' 'Stop' '--socket' '/state/hook.sock'"
    );
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote("a b; rm -rf /"), "'a b; rm -rf /'");
    // A path with a control character is refused, not quoted.
    let weird = LaunchFiles {
        runner_exe: Path::new("/opt/run\nner"),
        ..files()
    };
    assert!(hook_words(&weird, "claude", &key(), "Stop").is_err());
    assert!(claude_hook_command(&weird, &key(), "Stop").is_err());
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
    let command = claude_hook_command(&weird, &key(), "Stop").expect("command");
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("printf '%s\\n' {command}"))
        .output()
        .expect("sh");
    let words: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(words, hook_words(&weird, "claude", &key(), "Stop").expect("words"));
}
