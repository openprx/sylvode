//! ADR-0020 D2: legacy entry points warn, canonical entry points do not.
//!
//! Each test runs the shipped executables as real processes and reads what they wrote on
//! stdout and stderr. The warning texts' three required parts — the legacy name, its
//! replacement and the earliest removal release — are pinned as literal strings here rather
//! than taken from the library that builds them, so a test cannot pass by agreeing with a
//! broken text.

mod support;

use axum::{Json, Router};
use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

type BoxError = Box<dyn Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const TOKEN: &str = "opr_deprecation_bot_token";
const MCP_SERVER: &str = env!("CARGO_BIN_EXE_mcp-server");
const SYLVODE: &str = env!("CARGO_BIN_EXE_sylvode");

/// The removal clause every D2 notice carries.
const REMOVAL: &str = "not removed before Sylvode v2.0";

/// A stand-in API that answers every request with an empty successful envelope.
async fn spawn_api() -> Result<String, BoxError> {
    let router = Router::new()
        .fallback(|| async { Json(json!({"code": 0, "message": "success", "data": [{"id": "p1", "name": "Alpha"}]})) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(format!("http://{addr}"))
}

fn config(api_url: &str, transport: Option<&str>, bind_addr: Option<&str>) -> Result<ConfigFile, BoxError> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(TOKEN),
        workspace_id: WORKSPACE,
        transport,
        bind_addr,
    })
    .map_err(|error| error.to_string().into())
}

fn dir_of(config: &ConfigFile) -> Result<PathBuf, BoxError> {
    Ok(config.path().parent().ok_or("config has no parent")?.to_path_buf())
}

async fn run(binary: &str, cwd: &Path, args: &[&str], stderr: Stdio) -> Result<Output, BoxError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true);
    Ok(tokio::time::timeout(Duration::from_mins(1), command.spawn()?.wait_with_output()).await??)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// The stderr lines that are D2 notices.
fn notices(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|line| line.contains(REMOVAL)).collect()
}

/// `mcp-server <workspace command>` prints exactly one CLI notice on stderr naming the command,
/// its `sylvode` replacement and the removal release, leaves stdout and the exit code exactly
/// as `sylvode` produces them, and `sylvode` prints no notice — for a success and a failure.
#[tokio::test]
async fn legacy_workspace_commands_warn_once_on_stderr_and_sylvode_does_not() -> TestResult {
    let api_url = spawn_api().await?;
    let config = config(&api_url, Some("stdio"), None)?;
    let cwd = dir_of(&config)?;
    let path = config.path().display().to_string();
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        format!("http://{}", listener.local_addr()?)
    };

    for (args, subcommand, expected_exit) in [
        (vec!["projects", "list"], "projects list", 0),
        (vec!["labels", "list", "--format", "table"], "labels list", 0),
        (vec!["search", "anything"], "search", 0),
        (vec!["tools", "call", "--name", "projects.list"], "tools call", 0),
        (
            vec!["projects", "list", "--api-url", closed.as_str()],
            "projects list",
            1,
        ),
    ] {
        let mut full = args.clone();
        full.extend(["--config", path.as_str()]);
        let legacy = run(MCP_SERVER, &cwd, &full, Stdio::piped()).await?;
        let canonical = run(SYLVODE, &cwd, &full, Stdio::piped()).await?;

        let legacy_stderr = text(&legacy.stderr);
        let legacy_notices = notices(&legacy_stderr);
        assert_eq!(
            legacy_notices.len(),
            1,
            "mcp-server {args:?} must print exactly one notice:\n{legacy_stderr}"
        );
        let notice = legacy_notices.first().copied().unwrap_or_default();
        assert!(notice.contains(&format!("`mcp-server {subcommand}`")), "{notice}");
        assert!(notice.contains(&format!("`sylvode {subcommand}`")), "{notice}");
        assert!(notice.contains("deprecated"), "{notice}");

        let canonical_stderr = text(&canonical.stderr);
        assert!(
            notices(&canonical_stderr).is_empty() && !canonical_stderr.contains("deprecated"),
            "sylvode {args:?} printed a deprecation notice:\n{canonical_stderr}"
        );

        assert_eq!(legacy.status.code(), Some(expected_exit), "{args:?}: {legacy_stderr}");
        assert_eq!(
            legacy.status.code(),
            canonical.status.code(),
            "{args:?}: exit codes differ"
        );
        assert_eq!(legacy.stdout, canonical.stdout, "{args:?}: the notice changed stdout");
        assert!(
            !text(&legacy.stdout).contains(REMOVAL),
            "{args:?}: the notice reached stdout"
        );
        if expected_exit == 0 {
            assert!(!legacy.stdout.is_empty(), "{args:?}: success printed nothing");
        }
    }
    Ok(())
}

/// A notice that cannot be written does not fail the command: with stderr on `/dev/full`
/// every write to it fails, and the command still succeeds with the same stdout.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_unwritable_stderr_does_not_fail_the_legacy_command() -> TestResult {
    let api_url = spawn_api().await?;
    let config = config(&api_url, Some("stdio"), None)?;
    let cwd = dir_of(&config)?;
    let path = config.path().display().to_string();
    let args = ["projects", "list", "--config", path.as_str()];

    let full = std::fs::OpenOptions::new().write(true).open("/dev/full")?;
    let legacy = run(MCP_SERVER, &cwd, &args, Stdio::from(full)).await?;
    let canonical = run(SYLVODE, &cwd, &args, Stdio::piped()).await?;

    assert_eq!(
        legacy.status.code(),
        Some(0),
        "a failed notice write failed the command"
    );
    assert!(!legacy.stdout.is_empty());
    assert_eq!(legacy.stdout, canonical.stdout);
    Ok(())
}

fn free_port() -> Result<u16, BoxError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// `mcp-server serve --transport stdio`: stdout carries nothing but JSON-RPC frames and stderr
/// carries no notice.
#[tokio::test]
async fn serve_over_stdio_keeps_stdout_for_protocol_frames_and_prints_no_notice() -> TestResult {
    let api_url = spawn_api().await?;
    let config = config(&api_url, Some("stdio"), None)?;
    let cwd = dir_of(&config)?;
    let path = config.path().display().to_string();

    let mut child = Command::new(MCP_SERVER)
        .args(["serve", "--transport", "stdio", "--config", path.as_str()])
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut lines = BufReader::new(stdout).lines();

    let mut frames = Vec::new();
    for (id, method) in [(1, "initialize"), (2, "tools/list"), (3, "resources/list")] {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}});
        stdin.write_all(format!("{request}\n").as_bytes()).await?;
        stdin.flush().await?;
        let line = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await??
            .ok_or("stdio closed before answering")?;
        frames.push(line);
    }
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await??;
    // Whatever stdout still held after the answers were read must also be protocol frames.
    frames.extend(text(&output.stdout).lines().map(ToString::to_string));

    for frame in &frames {
        let parsed: Value =
            serde_json::from_str(frame).map_err(|error| format!("non-JSON on stdout: {frame:?}: {error}"))?;
        assert_eq!(
            parsed.get("jsonrpc"),
            Some(&json!("2.0")),
            "not a JSON-RPC frame: {frame}"
        );
    }
    assert_eq!(
        frames.len(),
        3,
        "stdout carried something besides the three answers: {frames:?}"
    );
    let stderr = text(&output.stderr);
    assert!(
        notices(&stderr).is_empty() && !stderr.contains("deprecated"),
        "serve over stdio printed a notice:\n{stderr}"
    );
    Ok(())
}

/// `mcp-server serve --transport http|sse` prints no notice either.
#[tokio::test]
async fn serve_over_http_and_sse_prints_no_notice() -> TestResult {
    let api_url = spawn_api().await?;
    for transport in ["http", "sse"] {
        let bind = format!("127.0.0.1:{}", free_port()?);
        let config = config(&api_url, Some(transport), Some(&bind))?;
        let cwd = dir_of(&config)?;
        let path = config.path().display().to_string();
        let mut child = Command::new(MCP_SERVER)
            .args([
                "serve",
                "--transport",
                transport,
                "--bind-addr",
                &bind,
                "--config",
                path.as_str(),
            ])
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let health = format!("http://{bind}/health");
        let mut healthy = false;
        for _ in 0..100 {
            if reqwest::get(&health)
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                healthy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        child.start_kill()?;
        let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await??;
        let stderr = text(&output.stderr);
        assert!(healthy, "{transport} never became healthy:\n{stderr}");
        assert!(
            notices(&stderr).is_empty() && !stderr.contains("deprecated"),
            "serve over {transport} printed a notice:\n{stderr}"
        );
        assert!(output.stdout.is_empty(), "serve over {transport} wrote to stdout");
    }
    Ok(())
}

/// The ADR-0020 D2 configuration notice's stable part.
const CONFIG_NOTICE: &str = "legacy configuration file config/openpr.toml was discovered by default";

/// A working directory holding `config/<file_name>` and nothing else, with the default
/// `[logging]` filter so a `warn` line from the binary is shown.
fn config_dir_with(file_name: &str, api_url: &str) -> Result<PathBuf, BoxError> {
    config_dir_with_format(file_name, api_url, "text")
}

/// [`config_dir_with`] with `logging.format` set to `format`.
fn config_dir_with_format(file_name: &str, api_url: &str, format: &str) -> Result<PathBuf, BoxError> {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "sylvode-config-notice-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("config"))?;
    std::fs::write(
        dir.join("config").join(file_name),
        format!(
            "[logging]\nformat = \"{format}\"\n\n[mcp]\napi_url = \"{api_url}\"\nbot_token = \"{TOKEN}\"\nworkspace_id = \"{WORKSPACE}\"\n"
        ),
    )?;
    Ok(dir)
}

fn config_notices(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.contains("legacy configuration file"))
        .collect()
}

/// Asserts exactly one configuration notice carrying the legacy name, the replacement and the
/// removal release, written as a `WARN` log line.
fn assert_one_config_notice(stderr: &str, what: &str) {
    let found = config_notices(stderr);
    assert_eq!(
        found.len(),
        1,
        "{what}: expected exactly one configuration notice:\n{stderr}"
    );
    let notice = found.first().copied().unwrap_or_default();
    assert!(notice.contains(CONFIG_NOTICE), "{what}: {notice}");
    assert!(notice.contains("config/sylvode.toml"), "{what}: {notice}");
    assert!(notice.contains(REMOVAL), "{what}: {notice}");
    assert!(notice.contains("WARN"), "{what}: not a warn log line: {notice}");
}

/// A Flow JSON envelope with its per-run `request_id` removed, for comparing two runs.
fn without_request_id(stdout: &[u8]) -> Result<Value, BoxError> {
    let mut envelope: Value = serde_json::from_slice(stdout)?;
    envelope
        .as_object_mut()
        .ok_or("the Flow output is not a JSON object")?
        .remove("request_id")
        .ok_or("the Flow output has no request_id")?;
    Ok(envelope)
}

const OBJECT: &str = "66666666-6666-4666-8666-666666666666";

/// Legacy configuration found by default discovery: one `warn` line per process from every
/// executable that loads configuration — `mcp-server` workspace commands, `sylvode` workspace
/// commands and `sylvode` Flow commands — on stderr only, with stdout identical to a run that
/// names the same file with `--config`.
#[tokio::test]
async fn default_discovery_of_the_legacy_config_warns_once_per_process_on_stderr() -> TestResult {
    let api_url = spawn_api().await?;
    let cwd = config_dir_with("openpr.toml", &api_url)?;

    for (binary, args) in [
        (MCP_SERVER, vec!["projects", "list"]),
        (SYLVODE, vec!["projects", "list"]),
        (SYLVODE, vec!["search", "anything", "--format", "table"]),
    ] {
        let discovered = run(binary, &cwd, &args, Stdio::piped()).await?;
        let stderr = text(&discovered.stderr);
        assert_eq!(discovered.status.code(), Some(0), "{binary} {args:?}: {stderr}");
        assert_one_config_notice(&stderr, &format!("{binary} {args:?}"));
        assert!(
            !text(&discovered.stdout).contains("legacy configuration"),
            "the notice reached stdout"
        );

        let mut explicit_args = args.clone();
        explicit_args.extend(["--config", "config/openpr.toml"]);
        let explicit = run(binary, &cwd, &explicit_args, Stdio::piped()).await?;
        assert_eq!(
            discovered.stdout, explicit.stdout,
            "{binary} {args:?}: the notice changed stdout"
        );
        assert!(!discovered.stdout.is_empty());
    }

    let flow_args = ["objects", "get", OBJECT];
    let discovered = run(SYLVODE, &cwd, &flow_args, Stdio::piped()).await?;
    let stderr = text(&discovered.stderr);
    assert_eq!(discovered.status.code(), Some(0), "{stderr}");
    assert_one_config_notice(&stderr, "sylvode objects get");
    let explicit = run(
        SYLVODE,
        &cwd,
        &["objects", "get", OBJECT, "--config", "config/openpr.toml"],
        Stdio::piped(),
    )
    .await?;
    assert_eq!(
        without_request_id(&discovered.stdout)?,
        without_request_id(&explicit.stdout)?
    );

    std::fs::remove_dir_all(&cwd)?;
    Ok(())
}

/// The stdio transport with default-discovered legacy configuration: the notice goes to
/// stderr, stdout carries only protocol frames, and there is still no CLI notice.
#[tokio::test]
async fn serve_over_stdio_with_discovered_legacy_config_warns_on_stderr_only() -> TestResult {
    let api_url = spawn_api().await?;
    let cwd = config_dir_with("openpr.toml", &api_url)?;
    let mut child = Command::new(MCP_SERVER)
        .args(["serve", "--transport", "stdio"])
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}});
    stdin.write_all(format!("{request}\n").as_bytes()).await?;
    stdin.flush().await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await??;

    let stdout = text(&output.stdout);
    let frames: Vec<&str> = stdout.lines().collect();
    assert_eq!(frames.len(), 1, "stdout: {stdout:?}");
    for frame in frames {
        let parsed: Value = serde_json::from_str(frame)?;
        assert_eq!(parsed.get("jsonrpc"), Some(&json!("2.0")), "{frame}");
    }
    let stderr = text(&output.stderr);
    assert_one_config_notice(&stderr, "serve --transport stdio");
    assert!(
        !stderr.contains("deprecated;"),
        "serve printed the CLI notice:\n{stderr}"
    );
    std::fs::remove_dir_all(&cwd)?;
    Ok(())
}

/// No notice when the operator named the legacy file explicitly, or when the canonical file is
/// the one discovered.
#[tokio::test]
async fn explicit_legacy_config_and_canonical_discovery_do_not_warn() -> TestResult {
    let api_url = spawn_api().await?;
    let legacy_cwd = config_dir_with("openpr.toml", &api_url)?;
    let canonical_cwd = config_dir_with("sylvode.toml", &api_url)?;

    let cases: [(&str, &Path, Vec<&str>); 6] = [
        (
            MCP_SERVER,
            &legacy_cwd,
            vec!["projects", "list", "--config", "config/openpr.toml"],
        ),
        (
            SYLVODE,
            &legacy_cwd,
            vec!["projects", "list", "--config", "config/openpr.toml"],
        ),
        (
            SYLVODE,
            &legacy_cwd,
            vec!["objects", "get", OBJECT, "--config", "config/openpr.toml"],
        ),
        (MCP_SERVER, &canonical_cwd, vec!["projects", "list"]),
        (SYLVODE, &canonical_cwd, vec!["projects", "list"]),
        (SYLVODE, &canonical_cwd, vec!["objects", "get", OBJECT]),
    ];
    for (binary, cwd, args) in cases {
        let output = run(binary, cwd, &args, Stdio::piped()).await?;
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{binary} {args:?}: {stderr}");
        assert!(
            config_notices(&stderr).is_empty(),
            "{binary} {args:?} in {} printed a configuration notice:\n{stderr}",
            cwd.display()
        );
    }
    std::fs::remove_dir_all(&legacy_cwd)?;
    std::fs::remove_dir_all(&canonical_cwd)?;
    Ok(())
}

/// A log line that cannot be written never fails a command (ADR-0020 D2: a warning must not
/// change the exit code, nor fail the command because writing it failed). With stderr on
/// `/dev/full` every write to it fails; each command must exit exactly as it does with a
/// writable stderr. The cases log through the global subscriber (`mcp-server` and `sylvode`
/// workspace commands), through the scoped stderr subscriber (a `sylvode` Flow command), and
/// with a `warn` event other than the configuration notice (`tools call` of an unknown tool,
/// with the canonical file discovered, so no notice is involved).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_unwritable_stderr_never_changes_the_exit_code_of_a_logging_command() -> TestResult {
    let api_url = spawn_api().await?;
    let legacy_cwd = config_dir_with("openpr.toml", &api_url)?;
    let canonical_cwd = config_dir_with("sylvode.toml", &api_url)?;
    let json_cwd = config_dir_with_format("openpr.toml", &api_url, "json")?;

    let cases: [(&str, &Path, Vec<&str>); 9] = [
        (MCP_SERVER, &legacy_cwd, vec!["projects", "list"]),
        (SYLVODE, &legacy_cwd, vec!["projects", "list"]),
        (SYLVODE, &legacy_cwd, vec!["objects", "get", OBJECT]),
        (MCP_SERVER, &canonical_cwd, vec!["tools", "call", "--name", "nope.nope"]),
        (SYLVODE, &canonical_cwd, vec!["tools", "call", "--name", "nope.nope"]),
        (MCP_SERVER, &legacy_cwd, vec!["serve", "--transport", "stdio"]),
        (MCP_SERVER, &json_cwd, vec!["projects", "list"]),
        (SYLVODE, &json_cwd, vec!["objects", "get", OBJECT]),
        (MCP_SERVER, &json_cwd, vec!["serve", "--transport", "stdio"]),
    ];
    for (binary, cwd, args) in cases {
        let writable = run(binary, cwd, &args, Stdio::piped()).await?;
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full")?;
        let unwritable = run(binary, cwd, &args, Stdio::from(full)).await?;
        assert_ne!(
            unwritable.status.code(),
            Some(101),
            "{binary} {args:?} panicked on an unwritable stderr"
        );
        assert_eq!(
            unwritable.status.code(),
            writable.status.code(),
            "{binary} {args:?}: an unwritable stderr changed the exit code (stderr when writable:\n{})",
            text(&writable.stderr)
        );
        assert!(
            !text(&writable.stderr).is_empty(),
            "{binary} {args:?} logs nothing, so it does not exercise a failed log write"
        );
    }
    std::fs::remove_dir_all(&legacy_cwd)?;
    std::fs::remove_dir_all(&canonical_cwd)?;
    std::fs::remove_dir_all(&json_cwd)?;
    Ok(())
}
