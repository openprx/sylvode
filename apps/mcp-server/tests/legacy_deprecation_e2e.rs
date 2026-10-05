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
