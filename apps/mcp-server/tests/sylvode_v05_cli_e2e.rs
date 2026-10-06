//! Shipped-binary coverage for the twelve v0.5 CLI rows in `cli-surface-v1.md`.

mod support;

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
};
use serde_json::{Value, json};
use std::{error::Error, path::Path, process::Output, sync::Arc, time::Duration};
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;

type TestResult = Result<(), Box<dyn Error>>;
type Calls = Arc<tokio::sync::Mutex<Vec<CapturedCall>>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const OBJECT: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "33333333-3333-4333-8333-333333333333";
const THIRD: &str = "44444444-4444-4444-8444-444444444444";
const TOKEN: &str = "opr_sylvode_v05_cli_e2e";

#[derive(Debug, Clone)]
struct CapturedCall {
    method: String,
    uri: String,
    body: Value,
}

async fn capture(State(calls): State<Calls>, request: Request) -> Json<Value> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let body = to_bytes(request.into_body(), 1_048_576)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let dry_run = body.get("dry_run").and_then(Value::as_bool) == Some(true);
    calls.lock().await.push(CapturedCall { method, uri, body });
    Json(if dry_run {
        json!({ "code": 0, "data": { "applied": false, "permission_changes": { "affected": [] } } })
    } else {
        json!({ "code": 0, "data": { "accepted": true } })
    })
}

async fn api() -> Result<(String, Calls), Box<dyn Error>> {
    let calls: Calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let router = Router::new().fallback(capture).with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{address}"), calls))
}

fn config(api_url: &str) -> Result<ConfigFile, Box<dyn Error>> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(TOKEN),
        workspace_id: WORKSPACE,
        transport: Some("stdio"),
        bind_addr: None,
    })
}

async fn run(config: &ConfigFile, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_sylvode"))
            .arg("--config")
            .arg(config.path())
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await??;
    Ok(output)
}

fn assert_success(output: &Output, command: &str) -> TestResult {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{command} failed: {stderr}");
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json_at(&envelope, "/schema_version")?, "sylvode.cli.v1");
    assert_eq!(json_at(&envelope, "/ok")?, true);
    assert_eq!(json_at(&envelope, "/command")?, command);
    Ok(())
}

fn json_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value, Box<dyn Error>> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("missing JSON pointer {pointer} in {value}").into())
}

fn write_patch_file(dir: &Path, contents: &str) -> Result<String, Box<dyn Error>> {
    let path = dir.join("patch.json");
    std::fs::write(&path, contents)?;
    Ok(path.to_string_lossy().to_string())
}

#[tokio::test]
async fn all_twelve_v05_lines_map_to_the_frozen_rest_shape() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let patch = write_patch_file(cwd, r#"[{"type":"set_title","title":"new"}]"#)?;

    let invocations: Vec<(&str, Vec<&str>)> = vec![
        (
            "objects.create",
            vec![
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "page",
                "--title",
                "Title",
                "--parent",
                OTHER,
                "--idempotency-key",
                "create-key",
            ],
        ),
        (
            "objects.patch",
            vec![
                "objects",
                "patch",
                OBJECT,
                "--patch-file",
                &patch,
                "--expected-frontier",
                "source-frontier",
                "--idempotency-key",
                "patch-key",
            ],
        ),
        (
            "objects.move",
            vec![
                "objects",
                "move",
                OBJECT,
                "--parent",
                OTHER,
                "--after",
                THIRD,
                "--expected-target-frontier",
                "target-frontier",
                "--idempotency-key",
                "move-key",
            ],
        ),
        ("objects.grants.get", vec!["objects", "grants", "get", OBJECT]),
        (
            "objects.grants.set",
            vec![
                "objects",
                "grants",
                "set",
                OBJECT,
                "--grant",
                "user:33333333-3333-4333-8333-333333333333=view",
                "--dry-run",
                "--idempotency-key",
                "grants-key",
            ],
        ),
        (
            "objects.inheritance.set",
            vec![
                "objects",
                "inheritance",
                "set",
                OBJECT,
                "--inherit",
                "false",
                "--dry-run",
                "--idempotency-key",
                "inheritance-key",
            ],
        ),
        (
            "objects.link",
            vec![
                "objects",
                "link",
                OBJECT,
                OTHER,
                "--kind",
                "related_to",
                "--idempotency-key",
                "link-key",
            ],
        ),
        (
            "objects.unlink",
            vec![
                "objects",
                "unlink",
                OBJECT,
                "--relation",
                THIRD,
                "--idempotency-key",
                "unlink-key",
            ],
        ),
        (
            "objects.diff",
            vec![
                "objects", "diff", OBJECT, "--from", "1", "--to", "2", "--render", "markdown",
            ],
        ),
        (
            "objects.relations",
            vec![
                "objects",
                "relations",
                OBJECT,
                "--direction",
                "both",
                "--kind",
                "related_to",
                "--limit",
                "25",
            ],
        ),
        (
            "objects.search",
            vec![
                "objects",
                "search",
                "--workspace",
                WORKSPACE,
                "--unprojected",
                "--query",
                "needle",
                "--freshness",
                "require-current",
            ],
        ),
        (
            "collab.projection-lag",
            vec![
                "collab",
                "projection-lag",
                "--workspace",
                WORKSPACE,
                "--project",
                OTHER,
                "--limit",
                "10",
            ],
        ),
    ];

    for (command, args) in invocations {
        assert_success(&run(&config, &args).await?, command)?;
    }

    let calls = calls.lock().await.clone();
    assert_eq!(calls.len(), 12);
    let [
        create,
        patch,
        move_object,
        grants_get,
        grants_set,
        inheritance_set,
        link,
        unlink,
        diff,
        relations,
        search,
        projection_lag,
    ] = calls.as_slice()
    else {
        return Err(format!("expected twelve captured calls, got {}", calls.len()).into());
    };
    assert_eq!(create.method, "POST");
    assert_eq!(create.uri, format!("/api/v1/workspaces/{WORKSPACE}/flow/objects"));
    assert_eq!(json_at(&create.body, "/parent_object_id")?, OTHER);
    assert_eq!(json_at(&patch.body, "/command/type")?, "semantic_patch");
    assert_eq!(json_at(&patch.body, "/expected_frontier")?, "source-frontier");
    assert_eq!(json_at(&move_object.body, "/command/type")?, "move_object");
    assert_eq!(json_at(&move_object.body, "/command/payload/target_object_id")?, OTHER);
    assert_eq!(
        json_at(&move_object.body, "/command/payload/expected_target_frontier")?,
        "target-frontier"
    );
    assert!(move_object.body.get("expected_frontier").is_none());
    assert_eq!(grants_get.method, "GET");
    assert_eq!(json_at(&grants_set.body, "/dry_run")?, true);
    assert_eq!(json_at(&inheritance_set.body, "/inherit_from_parent")?, false);
    assert_eq!(json_at(&link.body, "/command/payload/relation_type")?, "related_to");
    assert_eq!(json_at(&unlink.body, "/command/payload/relation_id")?, THIRD);
    assert!(diff.uri.contains("from_seq=1&to_seq=2&render=markdown"));
    assert!(
        relations
            .uri
            .contains("direction=both&relation_type=related_to&limit=25")
    );
    assert!(search.uri.contains("unprojected=true"));
    assert!(!search.uri.contains("all_visible"));
    assert!(projection_lag.uri.contains("project_id="));
    Ok(())
}

#[tokio::test]
async fn malformed_patch_file_exits_two_before_the_network() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let patch = write_patch_file(cwd, "not json")?;
    let output = run(
        &config,
        &[
            "objects",
            "patch",
            OBJECT,
            "--patch-file",
            &patch,
            "--idempotency-key",
            "patch-key",
        ],
    )
    .await?;
    assert_eq!(output.status.code(), Some(2));
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json_at(&envelope, "/ok")?, false);
    assert_eq!(json_at(&envelope, "/error/code")?, "usage_error");
    assert!(calls.lock().await.is_empty(), "invalid local JSON reached the API");
    Ok(())
}

/// Runs `sylvode` like [`run`], with a bound long enough for the CLI's own 30 s request timeout.
async fn run_unbounded_by_request_timeout(config: &ConfigFile, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    Ok(tokio::time::timeout(
        Duration::from_secs(90),
        Command::new(env!("CARGO_BIN_EXE_sylvode"))
            .arg("--config")
            .arg(config.path())
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await??)
}

/// `error-mapping-v1.md` "CLI 本地错误码 `network_error`": when the API gives no response at all
/// the Flow commands report the CLI-local code `network_error`, exit 9, `recoverable: true`,
/// `details: {"reason": "unreachable"}`, with a message naming the API URL and the cause, in
/// JSON on stdout and as `Error [network_error]: ...` on stderr in table format. Before, this was
/// `server_draining` with empty `details`, which violated `server_draining`'s required reason.
fn assert_network_error(output: &Output, api_url: &str, cause: &str) -> TestResult {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(9), "{stderr}");
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json_at(&envelope, "/schema_version")?, "sylvode.cli.v1");
    assert_eq!(json_at(&envelope, "/ok")?, false);
    assert_eq!(json_at(&envelope, "/command")?, "objects.get");
    assert_eq!(json_at(&envelope, "/error/code")?, "network_error");
    assert_eq!(json_at(&envelope, "/error/recoverable")?, true);
    assert_eq!(json_at(&envelope, "/error/details")?, &json!({"reason": "unreachable"}));
    let message = json_at(&envelope, "/error/message")?
        .as_str()
        .ok_or("error.message is not a string")?;
    assert!(
        message.starts_with("network failure: no response from the API at "),
        "{message}"
    );
    assert!(message.contains(api_url), "{message}");
    assert!(message.to_ascii_lowercase().contains(cause), "{message}");
    Ok(())
}

async fn assert_network_error_table(config: &ConfigFile) -> TestResult {
    let table = run_unbounded_by_request_timeout(config, &["--format", "table", "objects", "get", OBJECT]).await?;
    assert_eq!(table.status.code(), Some(9));
    assert!(table.stdout.is_empty(), "a failure wrote to stdout in table format");
    let stderr = String::from_utf8(table.stderr)?;
    assert!(
        stderr.contains("Error [network_error]: network failure: no response from the API"),
        "{stderr}"
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_connection_is_a_network_error() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let closed = format!("http://{}", listener.local_addr()?);
    drop(listener);
    let config = config(&closed)?;

    let output = run(&config, &["objects", "get", OBJECT]).await?;
    assert_network_error(&output, &closed, "connection refused")?;
    assert_network_error_table(&config).await
}

/// `.invalid` never resolves (RFC 6761), so this fails in name resolution without any network.
#[tokio::test]
async fn a_name_that_does_not_resolve_is_a_network_error() -> TestResult {
    let unresolvable = "http://sylvode-api.invalid:8080";
    let config = config(unresolvable)?;

    let output = run(&config, &["objects", "get", OBJECT]).await?;
    assert_network_error(&output, unresolvable, "dns error")?;
    assert_network_error_table(&config).await
}

/// An API that accepts the connection and never answers: the CLI's request timeout (30 s)
/// expires.
#[tokio::test]
async fn a_request_timeout_is_a_network_error() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let silent = format!("http://{}", listener.local_addr()?);
    let held = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let accepted = Arc::clone(&held);
    let acceptor = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            accepted.lock().await.push(socket);
        }
    });
    let config = config(&silent)?;

    let output = run_unbounded_by_request_timeout(&config, &["objects", "get", OBJECT]).await?;
    acceptor.abort();
    assert_network_error(&output, &silent, "timed out")
}
