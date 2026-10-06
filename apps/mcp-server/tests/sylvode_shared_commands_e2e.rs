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

/// The transport surface each request declared, canonical header then legacy header
/// (`mcp-surface-v1.md` "Transport → actor/origin"; ADR-0020 D3 sends both spellings).
type Surfaces = Arc<tokio::sync::Mutex<Vec<(String, String)>>>;

/// Starts the recording stand-in API and returns its base URL and its request log.
///
/// Every request is answered `200` with a `{code: 0}` envelope whose `data` echoes the
/// request and carries the fields the tools read from projects, work items and uploads —
/// except a request naming [`MISSING`], which gets the API's `404` error envelope.
async fn spawn_api() -> Result<(String, Seen, Surfaces), BoxError> {
    let seen: Seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let surfaces: Surfaces = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let surface_log = Arc::clone(&surfaces);
    let router = Router::new().fallback(move |request: Request| {
        let log = Arc::clone(&log);
        let surface_log = Arc::clone(&surface_log);
        async move {
            let declared = {
                let header = |name: &str| -> String {
                    request
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                };
                (header("x-sylvode-mcp-surface"), header("x-openpr-mcp-surface"))
            };
            surface_log.lock().await.push(declared);
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
    Ok((format!("http://{addr}"), seen, surfaces))
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
    surfaces: Surfaces,
    closed_url: String,
    upload: PathBuf,
    upload_missing: PathBuf,
}

impl Fixture {
    async fn new() -> Result<Self, BoxError> {
        let (api_url, seen, surfaces) = spawn_api().await?;
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
            surfaces,
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
    fixture.surfaces.lock().await.clear();
    let legacy = run(MCP_SERVER, fixture.dir()?, &full).await?;
    let legacy_requests = std::mem::take(&mut *fixture.seen.lock().await);
    let legacy_surfaces = std::mem::take(&mut *fixture.surfaces.lock().await);
    let canonical = run(SYLVODE, fixture.dir()?, &full).await?;
    let canonical_requests = std::mem::take(&mut *fixture.seen.lock().await);
    let canonical_surfaces = std::mem::take(&mut *fixture.surfaces.lock().await);

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
    assert_eq!(
        legacy_surfaces, canonical_surfaces,
        "{args:?}: the two executables declared different transport surfaces"
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
/// only the program name changed and the shared options' sentences about `serve`, which `sylvode`
/// does not have, rewritten (`cli::sylvode_help_prose`).
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
        mcp_server::cli::sylvode_help_prose(&legacy_help),
        canonical_help,
        "{group} help differs beyond the program name and the serve sentences"
    );
    let mentions_serve = canonical_help
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .any(|word| word == "serve");
    assert!(
        !mentions_serve,
        "sylvode {group} --help mentions serve:\n{canonical_help}"
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
    assert_same(
        &fixture,
        &[
            "projects",
            "create",
            "--key",
            "WPKEY",
            "--name",
            "Demo",
            "--description",
            "Desc",
        ],
        Expect::Success,
    )
    .await?;
    let created = run(
        SYLVODE,
        fixture.dir()?,
        &[
            "projects".to_string(),
            "create".to_string(),
            "--key".to_string(),
            "WPKEY".to_string(),
            "--name".to_string(),
            "Demo".to_string(),
            "--config".to_string(),
            fixture.config.path().display().to_string(),
        ],
    )
    .await?;
    assert_eq!(created.status.code(), Some(0));
    let requests = std::mem::take(&mut *fixture.seen.lock().await);
    let (method, _, body) = requests
        .iter()
        .find(|(method, _, _)| method == "POST")
        .ok_or("projects create sent no POST")?;
    let body: Value = serde_json::from_str(body)?;
    assert_eq!(method, "POST");
    assert_eq!(body.get("key"), Some(&json!("WPKEY")), "{body}");
    assert_eq!(body.get("name"), Some(&json!("Demo")), "{body}");
    assert_same(
        &fixture,
        &["projects", "create", "--key", "WPKEY", "--name", MISSING],
        Expect::ApiError,
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
/// `mcp-surface-v1.md`: "CLI 的 `tools call` 经 stdio-style configured bot 时使用
/// `surface=cli_tools_call`，保留 tool 名；native `sylvode` command 使用 `surface=cli`。"
///
/// The API binds every bot token to one surface and refuses any other, so the label is what
/// decides which token a command can use. Both executables, both header spellings.
#[tokio::test]
async fn tools_call_declares_the_cli_tools_call_surface_and_native_commands_declare_cli() -> TestResult {
    let fixture = Fixture::new().await?;
    for (args, expected) in [
        (&["tools", "call", "--name", "projects.list"][..], "cli_tools_call"),
        (&["projects", "list"][..], "cli"),
    ] {
        for binary in [MCP_SERVER, SYLVODE] {
            let mut full: Vec<String> = args.iter().map(ToString::to_string).collect();
            full.push("--config".to_string());
            full.push(fixture.config.path().display().to_string());
            fixture.surfaces.lock().await.clear();
            let output = run(binary, fixture.dir()?, &full).await?;
            assert_eq!(
                output.status.code(),
                Some(0),
                "{binary} {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let surfaces = std::mem::take(&mut *fixture.surfaces.lock().await);
            assert!(!surfaces.is_empty(), "{binary} {args:?}: no request reached the API");
            for (canonical, legacy) in &surfaces {
                assert_eq!(canonical, expected, "{binary} {args:?}: X-Sylvode-MCP-Surface");
                assert_eq!(legacy, expected, "{binary} {args:?}: X-OpenPR-MCP-Surface");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn sylvode_help_lists_all_fifteen_groups_and_no_serve() -> TestResult {
    let fixture = Fixture::new().await?;
    // A help flag before any group is top-level help, whichever group follows it.
    for args in [vec!["--help"], vec!["--help", "projects"], vec!["-h", "objects"]] {
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        let output = run(SYLVODE, fixture.dir()?, &args).await?;
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        let help = String::from_utf8(output.stdout)?;
        assert_top_level_help(&help, &args);
    }

    let serve = run(SYLVODE, fixture.dir()?, &["serve".to_string()]).await?;
    assert_eq!(serve.status.code(), Some(2), "sylvode must not offer serve");
    Ok(())
}

/// `sylvode`'s top-level help: the product description, all fifteen groups, no `serve`, and no
/// internal source documentation (rustdoc intra-doc links, which start with a bracket and a backtick).
fn assert_top_level_help(help: &str, args: &[String]) {
    assert!(
        help.starts_with("Sylvode Flow CLI and workspace commands"),
        "{args:?}:\n{help}"
    );
    assert!(!help.contains("[`"), "{args:?} prints rustdoc:\n{help}");
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
            "sylvode {args:?} does not list {group}:\n{help}"
        );
    }
    assert!(
        !help.lines().any(|line| line.trim_start().starts_with("serve ")),
        "{help}"
    );
}

/// `--version` and `-V` print the program name and the package version and exit 0 under both
/// names (ADR-0020 D5 lists `--version` among the allowed differences, so each names itself).
#[tokio::test]
async fn both_names_print_their_version() -> TestResult {
    let fixture = Fixture::new().await?;
    let version = env!("CARGO_PKG_VERSION");
    for (binary, name) in [(SYLVODE, "sylvode"), (MCP_SERVER, "mcp-server")] {
        for flag in ["--version", "-V"] {
            let output = run(binary, fixture.dir()?, &[flag.to_string()]).await?;
            assert_eq!(output.status.code(), Some(0), "{name} {flag}");
            assert_eq!(
                String::from_utf8(output.stdout)?,
                format!("{name} {version}\n"),
                "{name} {flag}"
            );
        }
    }
    Ok(())
}

/// Each parser's own help shows the product description, never the internal documentation of
/// the type that defines it.
#[tokio::test]
async fn group_help_never_prints_internal_documentation() -> TestResult {
    let fixture = Fixture::new().await?;
    for (binary, args) in [
        (SYLVODE, vec!["projects", "--help"]),
        (SYLVODE, vec!["objects", "--help"]),
        (MCP_SERVER, vec!["--help"]),
        (MCP_SERVER, vec!["projects", "--help"]),
    ] {
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        let output = run(binary, fixture.dir()?, &args).await?;
        assert_eq!(output.status.code(), Some(0), "{binary} {args:?}");
        let help = String::from_utf8(output.stdout)?;
        assert!(!help.contains("[`"), "{binary} {args:?} prints rustdoc:\n{help}");
    }
    Ok(())
}

/// `/dev/full`: every write fails with `ENOSPC`, the way a full disk or a dead terminal does.
#[cfg(target_os = "linux")]
fn unwritable() -> Result<Stdio, BoxError> {
    Ok(Stdio::from(std::fs::OpenOptions::new().write(true).open("/dev/full")?))
}

/// Runs `args` with the given stdout and stderr.
#[cfg(target_os = "linux")]
async fn run_redirected(
    binary: &str,
    cwd: &Path,
    args: &[&str],
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Output, BoxError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true);
    Ok(tokio::time::timeout(Duration::from_mins(1), command.spawn()?.wait_with_output()).await??)
}

/// An unwritable stream never panics a command (a panic exits 101). A failed workspace command
/// whose stderr cannot be written exits 1, exactly as with a writable stderr; a successful one
/// whose result cannot be written to stdout exits 1 instead of claiming success; both names
/// behave alike. The Flow renderer keeps a failure's own exit code and exits 1 for an
/// undeliverable success.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_unwritable_stream_never_panics_a_command() -> TestResult {
    let fixture = Fixture::new().await?;
    let dir = fixture.dir()?;
    let config = fixture.config.path().display().to_string();

    for binary in [MCP_SERVER, SYLVODE] {
        let failing = ["projects", "get", MISSING, "--config", &config];
        let writable = run_redirected(binary, dir, &failing, Stdio::piped(), Stdio::piped()).await?;
        let full = run_redirected(binary, dir, &failing, Stdio::piped(), unwritable()?).await?;
        assert_eq!(writable.status.code(), Some(1), "{binary}: {writable:?}");
        assert_eq!(full.status.code(), writable.status.code(), "{binary}: {full:?}");
        assert!(full.stdout.is_empty(), "{binary}: {full:?}");

        for format in ["json", "table"] {
            let listing = ["projects", "list", "--format", format, "--config", &config];
            let full = run_redirected(binary, dir, &listing, unwritable()?, Stdio::piped()).await?;
            let stderr = String::from_utf8_lossy(&full.stderr);
            assert_eq!(full.status.code(), Some(1), "{binary} {format}: {stderr}");
            assert!(!stderr.contains("panicked"), "{binary} {format}: {stderr}");
            assert!(
                stderr.contains("failed to write the result to stdout"),
                "{binary} {format}: {stderr}"
            );
        }
    }

    let flow_ok = ["features", "flow", "get", "--workspace", WORKSPACE, "--config", &config];
    let writable = run_redirected(SYLVODE, dir, &flow_ok, Stdio::piped(), Stdio::piped()).await?;
    assert_eq!(writable.status.code(), Some(0), "{writable:?}");
    let full = run_redirected(SYLVODE, dir, &flow_ok, unwritable()?, Stdio::piped()).await?;
    let stderr = String::from_utf8_lossy(&full.stderr);
    assert_eq!(full.status.code(), Some(1), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");

    let flow_usage = ["features", "flow", "get", "--workspace", "bad", "--config", &config];
    let full = run_redirected(SYLVODE, dir, &flow_usage, unwritable()?, Stdio::piped()).await?;
    assert_eq!(full.status.code(), Some(2), "{full:?}");
    let table_usage = [
        "features",
        "flow",
        "get",
        "--workspace",
        "bad",
        "--format",
        "table",
        "--config",
        &config,
    ];
    let full = run_redirected(SYLVODE, dir, &table_usage, Stdio::piped(), unwritable()?).await?;
    assert_eq!(full.status.code(), Some(2), "{full:?}");

    // An incomplete tool listing is a failure, reported without a panic.
    let listing = run_redirected(
        env!("CARGO_BIN_EXE_list-tools"),
        dir,
        &[],
        unwritable()?,
        Stdio::piped(),
    )
    .await?;
    assert_eq!(listing.status.code(), Some(1), "{listing:?}");
    Ok(())
}

/// `[logging] output = "stdout"` cannot be honoured by a process whose stdout is its result, so
/// logs go to stderr and the process says so. Under the default filter (no `logging.filter`) that
/// notice must reach stderr, while stdout stays exactly the command's JSON, under both names.
#[tokio::test]
async fn an_overridden_log_stream_is_reported_under_the_default_filter() -> TestResult {
    let fixture = Fixture::new().await?;
    let dir = fixture.dir()?;
    let source = std::fs::read_to_string(fixture.config.path())?;
    // The shared sections pin `filter = "error"`; this file keeps the binary's default filter.
    let logging = "[logging]\nfilter = \"error\"\nformat = \"text\"\n";
    let rewritten = source.replace(logging, "[logging]\nformat = \"text\"\noutput = \"stdout\"\n");
    assert_ne!(rewritten, source, "the fixture's [logging] section moved");
    let config = dir.join("stdout-logging.toml");
    std::fs::write(&config, rewritten)?;
    let config = config.display().to_string();

    let args = [
        "projects".to_string(),
        "list".to_string(),
        "--config".to_string(),
        config,
    ];
    let legacy = run(MCP_SERVER, dir, &args).await?;
    let canonical = run(SYLVODE, dir, &args).await?;
    for (name, output) in [("mcp-server", &legacy), ("sylvode", &canonical)] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{name}: {stderr}");
        assert!(
            stderr.contains("logging.output was overridden"),
            "{name}: the override notice is missing from stderr:\n{stderr}"
        );
        let stdout = String::from_utf8(output.stdout.clone())?;
        assert!(
            !stdout.contains("overridden"),
            "{name}: a log line reached stdout:\n{stdout}"
        );
        let parsed: Value = serde_json::from_str(&stdout)?;
        assert!(parsed.is_object(), "{name}: {stdout}");
    }
    assert_eq!(legacy.stdout, canonical.stdout);
    Ok(())
}
