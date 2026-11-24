//! Tool mode selection and the write switch against the shipped binary.
//!
//! `search` is the default of every transport. stdio takes the mode from `[mcp] tool_mode` and
//! `serve --tools` overrides it; HTTP takes it per request from `?tools=` or
//! `X-Sylvode-Tool-Mode` and falls back to the configured one. `[mcp] allow_write_call = false`
//! makes `sylvode_write` answer that writes are disabled. None of these checks needs the API:
//! listing and refusals never leave the process.

#[allow(dead_code)]
mod support;

use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::time::Duration;
use support::{McpSettings, write_config_with_extra_mcp_keys};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const BOT_TOKEN: &str = "opr_tool_mode_bot_token";
const API_URL: &str = "http://127.0.0.1:1";

fn names(response: &Value) -> Vec<String> {
    response
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn list_request() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
}

fn call_request(name: &str, arguments: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn result_text(response: &Value) -> String {
    response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---- stdio ----

/// Starts `serve` on stdio with `extra_keys` in `[mcp]` and `args` after `serve --config <file>`,
/// sends every request, and returns the answers in order.
async fn stdio(extra_keys: &str, args: &[&str], requests: &[Value]) -> Result<Vec<Value>, Box<dyn Error>> {
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: API_URL,
            bot_token: Some(BOT_TOKEN),
            workspace_id: Some(WORKSPACE),
            transport: Some("stdio"),
            bind_addr: None,
        },
        extra_keys,
    )?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("stdin")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("stdout")?).lines();
    let mut answers = Vec::new();
    for request in requests {
        stdin.write_all(format!("{request}\n").as_bytes()).await?;
        let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await??
            .ok_or("the server closed stdout")?;
        answers.push(serde_json::from_str(&line)?);
    }
    drop(stdin);
    let _ = child.kill().await;
    Ok(answers)
}

async fn stdio_listing(extra_keys: &str, args: &[&str]) -> Result<Vec<String>, Box<dyn Error>> {
    let answers = stdio(extra_keys, args, &[list_request()]).await?;
    Ok(names(answers.first().ok_or("no answer")?))
}

#[tokio::test]
async fn stdio_lists_search_mode_by_default_and_the_registry_when_configured() -> TestResult {
    assert_eq!(stdio_listing("", &[]).await?.len(), 9);
    assert_eq!(stdio_listing("tool_mode = \"search\"\n", &[]).await?.len(), 9);
    let full = stdio_listing("tool_mode = \"full\"\n", &[]).await?;
    assert_eq!(full.len(), 158);
    assert!(!full.contains(&"sylvode_search".to_string()));
    Ok(())
}

#[tokio::test]
async fn the_tools_flag_overrides_the_configured_mode_in_both_directions() -> TestResult {
    assert_eq!(
        stdio_listing("tool_mode = \"full\"\n", &["--tools", "search"])
            .await?
            .len(),
        9
    );
    assert_eq!(stdio_listing("", &["--tools", "full"]).await?.len(), 158);
    assert_eq!(
        stdio_listing("tool_mode = \"search\"\n", &["--tools", "full"])
            .await?
            .len(),
        158
    );
    Ok(())
}

#[tokio::test]
async fn stdio_search_mode_refuses_a_direct_registry_call_and_full_mode_accepts_the_name() -> TestResult {
    let direct = call_request("projects.list", &json!({}));
    let answers = stdio("", &[], std::slice::from_ref(&direct)).await?;
    let answer = answers.first().ok_or("no answer")?;
    assert_eq!(answer.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        result_text(answer),
        "Error: 'projects.list' is not listed in search mode; call it through sylvode_read or sylvode_write"
    );
    let answers = stdio("", &["--tools", "full"], &[direct]).await?;
    let text = result_text(answers.first().ok_or("no answer")?);
    assert!(!text.contains("is not listed in"), "{text}");
    Ok(())
}

#[tokio::test]
async fn stdio_initialize_carries_the_task_capability_and_instructions() -> TestResult {
    let answers = stdio(
        "",
        &[],
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})],
    )
    .await?;
    let answer = answers.first().ok_or("no answer")?;
    assert_eq!(
        answer.pointer("/result/capabilities/experimental/sylvode~1task-protocol/version"),
        Some(&json!(1))
    );
    assert!(
        answer
            .pointer("/result/instructions")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("sylvode_search")),
        "{answer}"
    );
    Ok(())
}

#[tokio::test]
async fn a_disabled_write_switch_answers_without_reaching_the_api() -> TestResult {
    let write = call_request(
        "sylvode_write",
        &json!({"tool": "labels.create", "arguments": {"name": "bug", "color": "#ff0000"}}),
    );
    let answers = stdio("allow_write_call = false\n", &[], &[write.clone(), list_request()]).await?;
    let answer = answers.first().ok_or("no answer")?;
    assert_eq!(answer.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        result_text(answer),
        "Error: writes are disabled for this bot by the runner configuration"
    );
    assert!(names(answers.get(1).ok_or("no listing")?).contains(&"sylvode_write".to_string()));
    // Writes allowed (the default): the call is attempted and fails only because the stand-in
    // address refuses connections, which is a different answer.
    let answers = stdio("", &[], &[write]).await?;
    let text = result_text(answers.first().ok_or("no answer")?);
    assert!(!text.contains("writes are disabled"), "{text}");
    Ok(())
}

async fn failing_serve(args: &[&str]) -> Result<(bool, String), Box<dyn Error>> {
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: API_URL,
            bot_token: Some(BOT_TOKEN),
            workspace_id: Some(WORKSPACE),
            transport: Some("stdio"),
            bind_addr: None,
        },
        "",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

#[tokio::test]
async fn tools_is_refused_with_flow_only_and_an_unknown_mode_is_refused() -> TestResult {
    let (ok, stderr) = failing_serve(&["--flow-only", "--tools", "full"]).await?;
    assert!(
        !ok && stderr.contains("--tools is not valid with serve --flow-only"),
        "{stderr}"
    );
    let (ok, stderr) = failing_serve(&["--tools", "everything"]).await?;
    assert!(!ok && stderr.contains("everything"), "{stderr}");
    Ok(())
}

#[tokio::test]
async fn an_invalid_configured_mode_refuses_to_start() -> TestResult {
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: API_URL,
            bot_token: Some(BOT_TOKEN),
            workspace_id: Some(WORKSPACE),
            transport: Some("stdio"),
            bind_addr: None,
        },
        "tool_mode = \"everything\"\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdin(Stdio::null())
        .output()
        .await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mcp.tool_mode must be search or full"));
    Ok(())
}

// ---- HTTP ----

struct Http {
    child: Child,
    base: String,
    _config: support::ConfigFile,
}

impl Http {
    async fn spawn(extra_keys: &str) -> Result<Self, Box<dyn Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let bind = listener.local_addr()?.to_string();
        drop(listener);
        let config = write_config_with_extra_mcp_keys(
            &McpSettings {
                api_url: API_URL,
                bot_token: None,
                workspace_id: Some(WORKSPACE),
                transport: Some("http"),
                bind_addr: Some(&bind),
            },
            extra_keys,
        )?;
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let http = Self {
            child,
            base: format!("http://{bind}"),
            _config: config,
        };
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if client
                .get(format!("{}/health", http.base))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(http);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("the HTTP server never became ready".into())
    }

    async fn post(
        &self,
        query: &str,
        header: Option<&str>,
        body: &Value,
    ) -> Result<(reqwest::StatusCode, Value), Box<dyn Error>> {
        let mut request = reqwest::Client::new()
            .post(format!("{}/mcp/rpc{query}", self.base))
            .bearer_auth(BOT_TOKEN)
            .json(body);
        if let Some(header) = header {
            request = request.header("X-Sylvode-Tool-Mode", header);
        }
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    async fn listing(&self, query: &str, header: Option<&str>) -> Result<usize, Box<dyn Error>> {
        let (status, body) = self.post(query, header, &list_request()).await?;
        assert_eq!(status, reqwest::StatusCode::OK, "{body}");
        Ok(names(&body).len())
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[tokio::test]
async fn http_lists_search_mode_unless_the_connection_asks_for_the_registry() -> TestResult {
    let server = Http::spawn("").await?;
    assert_eq!(server.listing("", None).await?, 9);
    assert_eq!(server.listing("?tools=search", None).await?, 9);
    assert_eq!(server.listing("?tools=full", None).await?, 158);
    assert_eq!(server.listing("", Some("full")).await?, 158);
    assert_eq!(server.listing("", Some("search")).await?, 9);
    assert_eq!(server.listing("?tools=full", Some("full")).await?, 158);
    Ok(())
}

#[tokio::test]
async fn the_configured_mode_is_the_http_default_and_a_connection_can_still_choose() -> TestResult {
    let server = Http::spawn("tool_mode = \"full\"\n").await?;
    assert_eq!(server.listing("", None).await?, 158);
    assert_eq!(server.listing("?tools=search", None).await?, 9);
    assert_eq!(server.listing("", Some("search")).await?, 9);
    Ok(())
}

#[tokio::test]
async fn http_refuses_an_unknown_or_conflicting_mode_and_a_direct_call_in_search_mode() -> TestResult {
    let server = Http::spawn("").await?;
    for (query, header) in [
        ("?tools=everything", None),
        ("", Some("everything")),
        ("?tools=full", Some("search")),
    ] {
        let (status, body) = server.post(query, header, &list_request()).await?;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{query} {header:?}");
        assert!(body.get("error").is_some(), "{body}");
    }
    let (status, body) = server
        .post("", None, &call_request("projects.list", &json!({})))
        .await?;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert!(result_text(&body).contains("is not listed in search mode"), "{body}");
    let (_, body) = server
        .post(
            "",
            None,
            &call_request("sylvode_write", &json!({"tool": "projects.list", "arguments": {}})),
        )
        .await?;
    assert!(
        result_text(&body).contains("only reads data; call it with sylvode_read"),
        "{body}"
    );
    Ok(())
}

#[tokio::test]
async fn the_write_switch_applies_to_http_connections() -> TestResult {
    let server = Http::spawn("allow_write_call = false\n").await?;
    let (_, body) = server
        .post(
            "",
            None,
            &call_request(
                "sylvode_write",
                &json!({"tool": "labels.create", "arguments": {"name": "b", "color": "#ff0000"}}),
            ),
        )
        .await?;
    assert_eq!(
        result_text(&body),
        "Error: writes are disabled for this bot by the runner configuration"
    );
    Ok(())
}

// ---- allow_write_call is a control in every mode and entry point ----

const WRITE_REFUSAL: &str = "Error: writes are disabled for this bot by the runner configuration";

fn direct_write() -> Value {
    call_request("labels.create", &json!({"name": "bug", "color": "#ff0000"}))
}

#[tokio::test]
async fn a_disabled_write_switch_holds_in_full_mode_on_stdio() -> TestResult {
    // The configured full mode and the flag both list the tool by name; the switch still refuses.
    for (extra, args) in [
        ("tool_mode = \"full\"\nallow_write_call = false\n", Vec::<&str>::new()),
        ("allow_write_call = false\n", vec!["--tools", "full"]),
    ] {
        let answers = stdio(extra, &args, &[direct_write(), list_request()]).await?;
        let answer = answers.first().ok_or("no answer")?;
        assert_eq!(
            answer.pointer("/result/isError"),
            Some(&json!(true)),
            "{extra} {args:?}"
        );
        assert_eq!(result_text(answer), WRITE_REFUSAL, "{extra} {args:?}");
        // The write tool is listed (full mode), and a read tool is not refused by the switch.
        assert!(names(answers.get(1).ok_or("no listing")?).contains(&"labels.create".to_string()));
        let read = stdio(extra, &args, &[call_request("projects.list", &json!({}))]).await?;
        assert!(!result_text(read.first().ok_or("no answer")?).contains("writes are disabled"));
    }
    Ok(())
}

#[tokio::test]
async fn a_disabled_write_switch_holds_in_full_mode_on_http() -> TestResult {
    let server = Http::spawn("allow_write_call = false\n").await?;
    let direct = direct_write();
    for (query, header) in [("?tools=full", None), ("", Some("full"))] {
        let (status, body) = server.post(query, header, &direct).await?;
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(result_text(&body), WRITE_REFUSAL, "{query} {header:?}");
    }
    let configured = Http::spawn("tool_mode = \"full\"\nallow_write_call = false\n").await?;
    let (_, body) = configured.post("", None, &direct).await?;
    assert_eq!(result_text(&body), WRITE_REFUSAL);
    Ok(())
}

async fn cli(extra_keys: &str, arguments: &[&str]) -> Result<(bool, String, String), Box<dyn Error>> {
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: API_URL,
            bot_token: Some(BOT_TOKEN),
            workspace_id: Some(WORKSPACE),
            transport: Some("stdio"),
            bind_addr: None,
        },
        extra_keys,
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_sylvode"))
        .arg("--config")
        .arg(config.path())
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .await?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

#[tokio::test]
async fn a_disabled_write_switch_holds_for_the_command_line() -> TestResult {
    let write_args = r##"{"name":"bug","color":"#ff0000"}"##;
    for arguments in [
        vec!["tools", "call", "--name", "labels.create", "--args-json", write_args],
        vec!["projects", "create", "--key", "ABC", "--name", "Example"],
    ] {
        let (ok, _, stderr) = cli("allow_write_call = false\n", &arguments).await?;
        assert!(!ok, "{arguments:?}");
        assert!(
            stderr.contains("writes are disabled for this bot by the runner configuration"),
            "{arguments:?}: {stderr}"
        );
    }
    // Writes allowed (the default): the call is attempted; the stand-in address refuses the
    // connection, which is a different failure.
    let (ok, _, stderr) = cli(
        "",
        &["tools", "call", "--name", "labels.create", "--args-json", write_args],
    )
    .await?;
    assert!(!ok && !stderr.contains("writes are disabled"), "{stderr}");
    // A read command and the task tools are not touched by the switch.
    let (_, _, stderr) = cli(
        "allow_write_call = false\n",
        &["tools", "call", "--name", "projects.list"],
    )
    .await?;
    assert!(!stderr.contains("writes are disabled"), "{stderr}");
    Ok(())
}

impl Http {
    /// A tools/list with any number of `X-Sylvode-Tool-Mode` header lines.
    async fn list_with_lines(
        &self,
        query: &str,
        lines: &[&str],
    ) -> Result<(reqwest::StatusCode, usize), Box<dyn Error>> {
        let mut request = reqwest::Client::new()
            .post(format!("{}/mcp/rpc{query}", self.base))
            .bearer_auth(BOT_TOKEN)
            .json(&list_request());
        for line in lines {
            request = request.header("X-Sylvode-Tool-Mode", *line);
        }
        let response = request.send().await?;
        let status = response.status();
        let body: Value = serde_json::from_str(&response.text().await?).unwrap_or(Value::Null);
        Ok((status, names(&body).len()))
    }
}

/// How a connection may spell its mode: case and blanks are tolerated, an identical repeat is
/// harmless, and every repeat that disagrees, or is not a mode, is a 400.
#[tokio::test]
async fn http_mode_spellings_and_repeats() -> TestResult {
    let server = Http::spawn("").await?;
    let ok = reqwest::StatusCode::OK;
    let bad = reqwest::StatusCode::BAD_REQUEST;
    for (query, lines, expected) in [
        ("?tools=FULL", vec![], (ok, 158)),
        ("?tools=%20full%20", vec![], (ok, 158)),
        ("?tools=full&tools=full", vec![], (ok, 158)),
        ("?other=1&tools=search&x=2", vec![], (ok, 9)),
        ("", vec!["full", "FULL"], (ok, 158)),
        ("?tools=full", vec!["full", "full"], (ok, 158)),
        ("?tools=search&tools=full", vec![], (bad, 0)),
        ("?tools=full&tools=search", vec![], (bad, 0)),
        ("?tools=", vec![], (bad, 0)),
        ("?tools=full&tools=", vec![], (bad, 0)),
        ("", vec!["search", "full"], (bad, 0)),
        ("", vec!["full", "everything"], (bad, 0)),
        ("", vec!["search, full"], (bad, 0)),
        ("?tools=full", vec!["full", "search"], (bad, 0)),
    ] {
        let answer = server.list_with_lines(query, &lines).await?;
        assert_eq!(answer.0, expected.0, "{query} {lines:?}");
        if expected.0 == ok {
            assert_eq!(answer.1, expected.1, "{query} {lines:?}");
        }
    }
    Ok(())
}
