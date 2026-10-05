//! ADR-0020 D5: `sylvode` carries the nine workspace command groups of `mcp-server`, as the same
//! commands.
//!
//! Every test here runs the two shipped executables — `mcp-server` and `sylvode` — as real
//! processes, with identical arguments and an identical configuration file, against one
//! recording stand-in for the Sylvode API, and compares what each process printed on stdout,
//! byte for byte, and the code it exited with. The stand-in echoes every request back in its
//! answer and records it, so equal stdout also means equal requests, and the recorded request
//! lists of the two runs are compared as well.
//!
//! Per group there is at least one read command, every write command the group has (labels,
//! sprints, search and operation-logs have none), one API error envelope and one network
//! failure. `<group> --help` is asserted to exit 0 under `sylvode` for each group on its own,
//! because the D2 warning tells `mcp-server` users to run exactly that command.

mod support;

use axum::{Json, Router, body::Bytes, extract::Request, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;

type BoxError = Box<dyn Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const TOKEN: &str = "opr_shared_commands_bot_token";
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const WORK_ITEM: &str = "33333333-3333-4333-8333-333333333333";
/// Any request whose path, query or body names this id is answered with an API error envelope.
const MISSING: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

/// One request as the stand-in saw it: method, path with query, body.
type Seen = Arc<tokio::sync::Mutex<Vec<(String, String, String)>>>;

/// Starts the recording stand-in API and returns its base URL and its request log.
///
/// Every request is answered `200` with a `{code: 0}` envelope whose `data` echoes the
/// request and carries the fields the tools read from projects, work items and uploads —
/// except a request naming [`MISSING`], which gets the API's `404` error envelope.
async fn spawn_api() -> Result<(String, Seen), BoxError> {
    let seen: Seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let router = Router::new().fallback(move |request: Request| {
        let log = Arc::clone(&log);
        async move {
            let method = request.method().to_string();
            let target = request
                .uri()
                .path_and_query()
                .map_or_else(String::new, ToString::to_string);
            let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                .await
                .unwrap_or_else(|_| Bytes::new());
            let body = normalized_multipart(&String::from_utf8_lossy(&body));
            log.lock().await.push((method.clone(), target.clone(), body.clone()));
            if target.contains(MISSING) || body.contains(MISSING) {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"code": 404, "message": "Resource not found", "data": null})),
                )
                    .into_response();
            }
            let echoed: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));
            Json(json!({
                "code": 0,
                "message": "success",
                "data": {
                    "method": method,
                    "path": target,
                    "body": echoed,
                    "id": "55555555-5555-4555-8555-555555555555",
                    "key": "DEMO",
                    "name": "Alpha",
                    "project_id": PROJECT,
                    "url": "/uploads/upload.txt",
                    "filename": "upload.txt",
                    "items": [{"id": "p1", "name": "Alpha"}, {"id": "p2", "name": "Beta"}]
                }
            }))
            .into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), seen))
}

/// A multipart body with its random boundary replaced by a fixed token, so two uploads of the
/// same file compare equal. Any other body is returned unchanged.
fn normalized_multipart(body: &str) -> String {
    match body.split_once("\r\n") {
        Some((first, _)) if first.starts_with("--") && first.len() > 2 => body.replace(first, "--BOUNDARY"),
        _ => body.to_string(),
    }
}

/// A loopback URL nothing listens on, for the network-failure path.
async fn closed_api_url() -> Result<String, BoxError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(format!("http://{addr}"))
}

struct Fixture {
    config: ConfigFile,
    seen: Seen,
    closed_url: String,
    upload: PathBuf,
    upload_missing: PathBuf,
}

impl Fixture {
    async fn new() -> Result<Self, BoxError> {
        let (api_url, seen) = spawn_api().await?;
        let config = write_config(&McpSettings {
            api_url: &api_url,
            bot_token: Some(TOKEN),
            workspace_id: WORKSPACE,
            transport: Some("stdio"),
            bind_addr: None,
        })
        .map_err(|error| error.to_string())?;
        let dir = config.path().parent().ok_or("config has no parent")?.to_path_buf();
        let upload = dir.join("upload.txt");
        std::fs::write(&upload, b"shared command upload\n")?;
        // The file name travels in the upload body, so naming it after MISSING makes the
        // stand-in answer the upload with its error envelope.
        let upload_missing = dir.join(format!("{MISSING}.txt"));
        std::fs::write(&upload_missing, b"upload refused by the stand-in\n")?;
        Ok(Self {
            config,
            seen,
            closed_url: closed_api_url().await?,
            upload,
            upload_missing,
        })
    }

    fn dir(&self) -> Result<&Path, BoxError> {
        Ok(self.config.path().parent().ok_or("config has no parent")?)
    }
}

async fn run(binary: &str, cwd: &Path, args: &[String]) -> Result<Output, BoxError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(tokio::time::timeout(Duration::from_mins(1), command.spawn()?.wait_with_output()).await??)
}

const MCP_SERVER: &str = env!("CARGO_BIN_EXE_mcp-server");
const SYLVODE: &str = env!("CARGO_BIN_EXE_sylvode");

/// What one case expects of both executables.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// Exit 0, non-empty stdout, and at least one request reached the API.
    Success,
    /// Exit 1 after the API answered with an error envelope.
    ApiError,
    /// Exit 1 without any request reaching the stand-in.
    LocalFailure,
}

/// Runs `args` under both executables and asserts identical stdout bytes, identical exit code,
/// identical requests, and the outcome `expect` describes.
async fn assert_same(fixture: &Fixture, args: &[&str], expect: Expect) -> TestResult {
    let mut full: Vec<String> = args.iter().map(ToString::to_string).collect();
    full.push("--config".to_string());
    full.push(fixture.config.path().display().to_string());

    fixture.seen.lock().await.clear();
    let legacy = run(MCP_SERVER, fixture.dir()?, &full).await?;
    let legacy_requests = std::mem::take(&mut *fixture.seen.lock().await);
    let canonical = run(SYLVODE, fixture.dir()?, &full).await?;
    let canonical_requests = std::mem::take(&mut *fixture.seen.lock().await);

    let shown = |output: &Output| {
        format!(
            "exit={:?}\nstdout={}\nstderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    };
    assert_eq!(
        legacy.stdout,
        canonical.stdout,
        "{args:?}: stdout differs\n--- mcp-server\n{}\n--- sylvode\n{}",
        shown(&legacy),
        shown(&canonical)
    );
    assert_eq!(
        legacy.status.code(),
        canonical.status.code(),
        "{args:?}: exit code differs\n--- mcp-server\n{}\n--- sylvode\n{}",
        shown(&legacy),
        shown(&canonical)
    );
    assert_eq!(
        legacy_requests, canonical_requests,
        "{args:?}: the API saw different requests"
    );

    let expected_code = match expect {
        Expect::Success => 0,
        Expect::ApiError | Expect::LocalFailure => 1,
    };
    assert_eq!(
        canonical.status.code(),
        Some(expected_code),
        "{args:?}: unexpected outcome\n{}",
        shown(&canonical)
    );
    match expect {
        Expect::Success => {
            assert!(!canonical.stdout.is_empty(), "{args:?}: success printed nothing");
            assert!(!canonical_requests.is_empty(), "{args:?}: success made no API call");
        }
        Expect::ApiError => {
            assert!(canonical.stdout.is_empty(), "{args:?}: an error wrote to stdout");
            assert!(
                canonical_requests
                    .iter()
                    .any(|(_, target, body)| target.contains(MISSING) || body.contains(MISSING)),
                "{args:?}: the error envelope was never requested"
            );
        }
        Expect::LocalFailure => {
            assert!(canonical.stdout.is_empty(), "{args:?}: a failure wrote to stdout");
            assert!(
                canonical_requests.is_empty(),
                "{args:?}: a local failure reached the API"
            );
        }
    }
    Ok(())
}

/// `sylvode <group> --help` exits 0 and prints the help `mcp-server <group> --help` prints, with
/// only the program name changed.
async fn assert_help(fixture: &Fixture, group: &str) -> TestResult {
    let args = vec![group.to_string(), "--help".to_string()];
    let canonical = run(SYLVODE, fixture.dir()?, &args).await?;
    assert_eq!(
        canonical.status.code(),
        Some(0),
        "sylvode {group} --help failed: {}",
        String::from_utf8_lossy(&canonical.stderr)
    );
    let canonical_help = String::from_utf8(canonical.stdout)?;
    assert!(
        canonical_help.contains(&format!("Usage: sylvode {group}")),
        "{canonical_help}"
    );
    let legacy = run(MCP_SERVER, fixture.dir()?, &args).await?;
    assert_eq!(legacy.status.code(), Some(0));
    let legacy_help = String::from_utf8(legacy.stdout)?;
    assert_eq!(
        legacy_help.replace("mcp-server", "sylvode"),
        canonical_help,
        "{group} help differs beyond the program name"
    );
    Ok(())
}

/// The network-failure path: the same command against a port nothing listens on.
async fn assert_network_failure(fixture: &Fixture, args: &[&str]) -> TestResult {
    let mut with_url: Vec<&str> = args.to_vec();
    with_url.push("--api-url");
    with_url.push(&fixture.closed_url);
    assert_same(fixture, &with_url, Expect::LocalFailure).await
}

#[tokio::test]
async fn projects_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "projects").await?;
    assert_same(&fixture, &["projects", "list"], Expect::Success).await?;
    assert_same(&fixture, &["projects", "list", "--format", "table"], Expect::Success).await?;
    assert_same(&fixture, &["projects", "get", PROJECT], Expect::Success).await?;
    assert_same(
        &fixture,
        &[
            "projects",
            "list",
            "--workspace-id",
            "44444444-4444-4444-8444-444444444444",
        ],
        Expect::Success,
    )
    .await?;
    // `projects create` offers no `--key`, which the tool requires, so the write is refused
    // before any request under both names; the refusal itself must be identical.
    assert_same(
        &fixture,
        &["projects", "create", "--name", "Demo", "--description", "Desc"],
        Expect::LocalFailure,
    )
    .await?;
    assert_same(&fixture, &["projects", "get", MISSING], Expect::ApiError).await?;
    assert_network_failure(&fixture, &["projects", "list"]).await
}

#[tokio::test]
async fn work_items_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "work-items").await?;
    assert_same(
        &fixture,
        &["work-items", "list", "--project", PROJECT, "--state", "todo"],
        Expect::Success,
    )
    .await?;
    assert_same(&fixture, &["work-items", "get", "PRX-1"], Expect::Success).await?;
    assert_same(&fixture, &["work-items", "search", "--query", "bug"], Expect::Success).await?;
    assert_same(
        &fixture,
        &[
            "work-items",
            "create",
            "--project",
            PROJECT,
            "--title",
            "T",
            "--priority",
            "high",
        ],
        Expect::Success,
    )
    .await?;
    assert_same(
        &fixture,
        &[
            "work-items",
            "update",
            WORK_ITEM,
            "--state",
            "done",
            "--format",
            "table",
        ],
        Expect::Success,
    )
    .await?;
    assert_same(&fixture, &["work-items", "get", MISSING], Expect::ApiError).await?;
    assert_network_failure(&fixture, &["work-items", "search", "--query", "bug"]).await
}

#[tokio::test]
async fn comments_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "comments").await?;
    assert_same(
        &fixture,
        &["comments", "list", "--work-item", WORK_ITEM],
        Expect::Success,
    )
    .await?;
    assert_same(
        &fixture,
        &["comments", "create", "--work-item", WORK_ITEM, "--content", "hello"],
        Expect::Success,
    )
    .await?;
    assert_same(
        &fixture,
        &["comments", "create", "--work-item", MISSING, "--content", "hello"],
        Expect::ApiError,
    )
    .await?;
    assert_network_failure(&fixture, &["comments", "list", "--work-item", WORK_ITEM]).await
}

#[tokio::test]
async fn labels_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "labels").await?;
    assert_same(&fixture, &["labels", "list"], Expect::Success).await?;
    assert_same(
        &fixture,
        &["labels", "list", "--project", PROJECT, "--format", "table"],
        Expect::Success,
    )
    .await?;
    assert_same(&fixture, &["labels", "list", "--project", MISSING], Expect::ApiError).await?;
    assert_network_failure(&fixture, &["labels", "list"]).await
}

#[tokio::test]
async fn sprints_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "sprints").await?;
    assert_same(&fixture, &["sprints", "list", "--project", PROJECT], Expect::Success).await?;
    assert_same(&fixture, &["sprints", "list", "--project", MISSING], Expect::ApiError).await?;
    assert_network_failure(&fixture, &["sprints", "list", "--project", PROJECT]).await
}

#[tokio::test]
async fn search_is_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "search").await?;
    assert_same(&fixture, &["search", "anything"], Expect::Success).await?;
    assert_same(&fixture, &["search", "anything", "--format", "table"], Expect::Success).await?;
    assert_same(&fixture, &["search", MISSING], Expect::ApiError).await?;
    assert_network_failure(&fixture, &["search", "anything"]).await
}

#[tokio::test]
async fn files_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "files").await?;
    let upload = fixture.upload.display().to_string();
    let upload_missing = fixture.upload_missing.display().to_string();
    assert_same(&fixture, &["files", "upload", "--file", &upload], Expect::Success).await?;
    assert_same(
        &fixture,
        &["files", "upload", "--file", &upload_missing],
        Expect::ApiError,
    )
    .await?;
    assert_same(
        &fixture,
        &["files", "upload", "--file", "/nonexistent/sylvode-upload"],
        Expect::LocalFailure,
    )
    .await?;
    assert_network_failure(&fixture, &["files", "upload", "--file", &upload]).await
}

#[tokio::test]
async fn operation_logs_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "operation-logs").await?;
    assert_same(&fixture, &["operation-logs", "list", "--limit", "5"], Expect::Success).await?;
    assert_same(
        &fixture,
        &["operation-logs", "list", "--bot-id", MISSING],
        Expect::ApiError,
    )
    .await?;
    assert_network_failure(&fixture, &["operation-logs", "list"]).await
}

#[tokio::test]
async fn tools_are_the_same_command_under_both_names() -> TestResult {
    let fixture = Fixture::new().await?;
    assert_help(&fixture, "tools").await?;
    assert_same(&fixture, &["tools", "call", "--name", "projects.list"], Expect::Success).await?;
    let create = json!({"project_id": PROJECT, "title": "from tools"}).to_string();
    assert_same(
        &fixture,
        &["tools", "call", "--name", "work_items.create", "--args-json", &create],
        Expect::Success,
    )
    .await?;
    let missing = json!({"project_id": MISSING}).to_string();
    assert_same(
        &fixture,
        &["tools", "call", "--name", "projects.get", "--args-json", &missing],
        Expect::ApiError,
    )
    .await?;
    assert_same(
        &fixture,
        &["tools", "call", "--name", "projects.list", "--args-json", "[]"],
        Expect::LocalFailure,
    )
    .await?;
    assert_network_failure(&fixture, &["tools", "call", "--name", "projects.list"]).await
}

/// `sylvode --help` names all fifteen groups, and `serve` stays `mcp-server`'s alone.
#[tokio::test]
async fn sylvode_help_lists_all_fifteen_groups_and_no_serve() -> TestResult {
    let fixture = Fixture::new().await?;
    let output = run(SYLVODE, fixture.dir()?, &["--help".to_string()]).await?;
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8(output.stdout)?;
    for group in [
        "features",
        "objects",
        "collections",
        "records",
        "collab",
        "deliveries",
        "projects",
        "work-items",
        "comments",
        "labels",
        "sprints",
        "search",
        "files",
        "operation-logs",
        "tools",
    ] {
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with(&format!("{group} "))),
            "sylvode --help does not list {group}:\n{help}"
        );
    }
    assert!(
        !help.lines().any(|line| line.trim_start().starts_with("serve ")),
        "{help}"
    );

    let serve = run(SYLVODE, fixture.dir()?, &["serve".to_string()]).await?;
    assert_eq!(serve.status.code(), Some(2), "sylvode must not offer serve");
    Ok(())
}
