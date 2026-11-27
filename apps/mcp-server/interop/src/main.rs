//! `mcp-interop`: acceptance checks of the Sylvode MCP server for protocol revision 2026-07-28.
//!
//! Every subcommand prints one JSON evidence line on stdout and exits non-zero when the check
//! fails. Where the official SDK models the case (`DiscoverResult`), the SDK type parses the
//! answer; the negative cases are raw JSON-RPC, because an SDK client never sends them.
//!
//! ```text
//! mcp-interop <check> (--http <base-url> | --stdio -- <command> [args...])
//! ```
//!
//! The bearer token of the HTTP transport is read from the environment variable named by
//! `--token-env` (default `MCP_INTEROP_TOKEN`); it is never printed.
//!
//! Checks: `discover` (T4, T5), `unsupported-version` (T6), `header-mismatch` (T7, HTTP only),
//! `read-missing` (T11), `legacy-init` (T1 shape, T19).

use rmcp::model::DiscoverResult;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

type Failure = Box<dyn std::error::Error>;
type Outcome = Result<Value, Failure>;

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const MODERN: &str = "2026-07-28";

/// One answer: the HTTP status (HTTP only) and the JSON-RPC message.
struct Answer {
    status: Option<u16>,
    body: Value,
}

enum Transport {
    Http {
        client: reqwest::Client,
        url: String,
        token: String,
    },
    Stdio {
        _child: Child,
        stdin: ChildStdin,
        lines: Lines<BufReader<ChildStdout>>,
    },
}

impl Transport {
    /// Sends `body`. `headers` are added on HTTP and ignored on stdio.
    async fn send(&mut self, body: &Value, headers: &[(String, String)]) -> Result<Answer, Failure> {
        match self {
            Self::Http { client, url, token } => {
                let mut request = client.post(url.as_str()).bearer_auth(token.as_str()).json(body);
                for (name, value) in headers {
                    request = request.header(name.as_str(), value.as_str());
                }
                let response = request.send().await?;
                let status = response.status().as_u16();
                let text = response.text().await?;
                let body = if text.trim().is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&text)?
                };
                Ok(Answer {
                    status: Some(status),
                    body,
                })
            }
            Self::Stdio { stdin, lines, .. } => {
                stdin.write_all(format!("{body}\n").as_bytes()).await?;
                stdin.flush().await?;
                let line = tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
                    .await??
                    .ok_or("the server closed stdout")?;
                Ok(Answer {
                    status: None,
                    body: serde_json::from_str(&line)?,
                })
            }
        }
    }

    const fn is_http(&self) -> bool {
        matches!(self, Self::Http { .. })
    }
}

/// A modern request with the three HTTP headers a modern client sends.
fn modern(id: i64, method: &str, params: Value) -> (Value, Vec<(String, String)>) {
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        fields.insert(
            "_meta".to_string(),
            json!({
                META_VERSION: MODERN,
                "io.modelcontextprotocol/clientInfo": {"name": "mcp-interop", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
    }
    let headers = vec![
        ("MCP-Protocol-Version".to_string(), MODERN.to_string()),
        ("Mcp-Method".to_string(), method.to_string()),
    ];
    (json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}), headers)
}

fn check(condition: bool, what: &str, evidence: &Value) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(format!("{what}: {evidence}").into())
    }
}

/// T4/T5: one discover, parsed by the SDK type, then a modern `tools/list` without `initialize`.
async fn discover(transport: &mut Transport) -> Outcome {
    let (body, mut headers) = modern(1, "server/discover", json!({}));
    headers.retain(|(name, _)| name != "Mcp-Method");
    headers.push(("Mcp-Method".to_string(), "server/discover".to_string()));
    let answer = transport.send(&body, &headers).await?;
    check(answer.status.is_none_or(|status| status == 200), "discover status", &answer.body)?;
    let result = answer.body.get("result").ok_or("discover returned no result")?;
    let parsed: DiscoverResult = serde_json::from_value(result.clone())
        .map_err(|error| format!("the SDK cannot parse the discover result: {error}: {result}"))?;
    check(
        parsed.supported_versions.iter().any(|version| version.as_str() == MODERN),
        "supportedVersions lists 2026-07-28",
        result,
    )?;
    check(parsed.ttl_ms > 0, "ttlMs is set", result)?;
    let server = parsed.server_info();

    let (list, headers) = modern(2, "tools/list", json!({}));
    let listed = transport.send(&list, &headers).await?;
    let tools = listed
        .body
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    check(tools > 0, "modern tools/list lists tools", &listed.body)?;
    check(
        listed.body.pointer("/result/resultType") == Some(&json!("complete")),
        "modern tools/list resultType",
        &listed.body,
    )?;
    Ok(json!({
        "check": "discover",
        "server": server.map(|info| info.name),
        "ttlMs": parsed.ttl_ms,
        "instructions": parsed.instructions.is_some(),
        "tools": tools,
    }))
}

/// T6: an unknown modern version is `-32022` with `supported` and `requested` (HTTP: status 400).
async fn unsupported_version(transport: &mut Transport) -> Outcome {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list",
                      "params": {"_meta": {META_VERSION: "1900-01-01"}}});
    let headers = vec![
        ("MCP-Protocol-Version".to_string(), "1900-01-01".to_string()),
        ("Mcp-Method".to_string(), "tools/list".to_string()),
    ];
    let answer = transport.send(&body, &headers).await?;
    check(answer.body.pointer("/error/code") == Some(&json!(-32022)), "code -32022", &answer.body)?;
    check(
        answer.body.pointer("/error/data/requested") == Some(&json!("1900-01-01")),
        "data.requested",
        &answer.body,
    )?;
    check(
        answer
            .body
            .pointer("/error/data/supported")
            .and_then(Value::as_array)
            .is_some_and(|list| list.contains(&json!(MODERN))),
        "data.supported lists 2026-07-28",
        &answer.body,
    )?;
    if transport.is_http() {
        check(answer.status == Some(400), "HTTP 400", &answer.body)?;
    }
    Ok(json!({"check": "unsupported-version", "status": answer.status, "supported": answer.body.pointer("/error/data/supported")}))
}

/// T7: `Mcp-Name` that differs from `params.name` is `-32020` with `data.header` (HTTP: 400).
async fn header_mismatch(transport: &mut Transport) -> Outcome {
    if !transport.is_http() {
        return Err("header-mismatch needs --http".into());
    }
    let (body, mut headers) = modern(1, "tools/call", json!({"name": "sylvode_overview", "arguments": {}}));
    headers.push(("Mcp-Name".to_string(), "some_other_tool".to_string()));
    let answer = transport.send(&body, &headers).await?;
    check(answer.status == Some(400), "HTTP 400", &answer.body)?;
    check(answer.body.pointer("/error/code") == Some(&json!(-32020)), "code -32020", &answer.body)?;
    check(
        answer.body.pointer("/error/data/header") == Some(&json!("Mcp-Name")),
        "data.header",
        &answer.body,
    )?;
    Ok(json!({"check": "header-mismatch", "status": answer.status, "data": answer.body.pointer("/error/data")}))
}

/// T11: a missing issue is `-32602` "Resource not found" in the modern era and `-32002` in the
/// legacy era, with `data.uri`, and the connection keeps working.
async fn read_missing(transport: &mut Transport) -> Outcome {
    let uri = "sylvode://issues/NOPE-999999";
    let (body, mut headers) = modern(1, "resources/read", json!({"uri": uri}));
    headers.push(("Mcp-Name".to_string(), uri.to_string()));
    let modern_answer = transport.send(&body, &headers).await?;
    check(
        modern_answer.body.pointer("/error/code") == Some(&json!(-32602)),
        "modern code -32602",
        &modern_answer.body,
    )?;
    check(
        modern_answer.body.pointer("/error/message") == Some(&json!("Resource not found")),
        "modern message",
        &modern_answer.body,
    )?;
    check(
        modern_answer.body.pointer("/error/data/uri") == Some(&json!(uri)),
        "modern data.uri",
        &modern_answer.body,
    )?;
    let legacy = json!({"jsonrpc": "2.0", "id": 2, "method": "resources/read", "params": {"uri": uri}});
    let legacy_answer = transport.send(&legacy, &[]).await?;
    check(
        legacy_answer.body.pointer("/error/code") == Some(&json!(-32002)),
        "legacy code -32002",
        &legacy_answer.body,
    )?;
    check(
        legacy_answer.body.pointer("/error/data/uri") == Some(&json!(uri)),
        "legacy data.uri",
        &legacy_answer.body,
    )?;
    let (list, headers) = modern(3, "tools/list", json!({}));
    let after = transport.send(&list, &headers).await?;
    check(after.body.pointer("/result/tools").is_some(), "connection still works", &after.body)?;
    Ok(json!({"check": "read-missing", "modern": modern_answer.body.get("error"), "legacy": legacy_answer.body.get("error")}))
}

/// T1/T19 shape: a legacy `initialize` echoes the requested version, then a legacy `tools/list`
/// answers without any modern field, on the same connection as a modern request.
async fn legacy_init(transport: &mut Transport) -> Outcome {
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                      "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "mcp-interop-legacy", "version": "1"}}});
    let answer = transport.send(&init, &[]).await?;
    check(
        answer.body.pointer("/result/protocolVersion") == Some(&json!("2025-06-18")),
        "initialize echoes the version",
        &answer.body,
    )?;
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let legacy = transport.send(&list, &[]).await?;
    check(legacy.body.pointer("/result/tools").is_some(), "legacy tools/list", &legacy.body)?;
    check(legacy.body.pointer("/result/resultType").is_none(), "no modern field in a legacy result", &legacy.body)?;
    let (modern_list, headers) = modern(3, "tools/list", json!({}));
    let modern_answer = transport.send(&modern_list, &headers).await?;
    check(
        modern_answer.body.pointer("/result/resultType") == Some(&json!("complete")),
        "modern request on the same connection",
        &modern_answer.body,
    )?;
    Ok(json!({"check": "legacy-init", "protocolVersion": answer.body.pointer("/result/protocolVersion")}))
}

fn usage() -> Failure {
    "usage: mcp-interop <discover|unsupported-version|header-mismatch|read-missing|legacy-init> \
     [--token-env NAME] (--http <base-url> | --stdio -- <command> [args...])"
        .into()
}

async fn connect(arguments: &[String]) -> Result<Transport, Failure> {
    let mut token_env = "MCP_INTEROP_TOKEN".to_string();
    let mut http = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--token-env" => {
                token_env = arguments.get(index + 1).ok_or_else(usage)?.clone();
                index += 2;
            }
            "--http" => {
                http = Some(arguments.get(index + 1).ok_or_else(usage)?.clone());
                index += 2;
            }
            "--stdio" => {
                let command = arguments.get(index + 1..).ok_or_else(usage)?;
                let command = command.strip_prefix(&["--".to_string()]).unwrap_or(command);
                let (program, rest) = command.split_first().ok_or_else(usage)?;
                let mut child = Command::new(program)
                    .args(rest)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()?;
                let stdin = child.stdin.take().ok_or("no stdin")?;
                let lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
                return Ok(Transport::Stdio {
                    _child: child,
                    stdin,
                    lines,
                });
            }
            _ => return Err(usage()),
        }
    }
    let base = http.ok_or_else(usage)?;
    let token = std::env::var(&token_env).map_err(|_| format!("environment variable {token_env} is not set"))?;
    Ok(Transport::Http {
        client: reqwest::Client::new(),
        url: format!("{}/mcp/rpc", base.trim_end_matches('/')),
        token,
    })
}

async fn run() -> Outcome {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (check_name, rest) = arguments.split_first().ok_or_else(usage)?;
    let mut transport = connect(rest).await?;
    match check_name.as_str() {
        "discover" => discover(&mut transport).await,
        "unsupported-version" => unsupported_version(&mut transport).await,
        "header-mismatch" => header_mismatch(&mut transport).await,
        "read-missing" => read_missing(&mut transport).await,
        "legacy-init" => legacy_init(&mut transport).await,
        _ => Err(usage()),
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(mut evidence) => {
            if let Some(fields) = evidence.as_object_mut() {
                fields.insert("ok".to_string(), json!(true));
            }
            println!("{evidence}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({"ok": false, "error": error.to_string()}));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(without: Option<&str>) -> Value {
        let mut result = json!({
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": {"tools": {}, "resources": {"subscribe": false, "listChanged": false},
                             "extensions": {"io.sylvode/task-protocol": {"version": 1}}},
            "instructions": "x",
            "ttlMs": 3_600_000,
            "cacheScope": "private",
            "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "sylvode-mcp-server", "version": "0.5.0"}},
        });
        if let (Some(key), Some(fields)) = (without, result.as_object_mut()) {
            fields.remove(key);
        }
        result
    }

    /// T5: a discover result without `ttlMs` does not parse, which is why sv must send it; a full
    /// one does, and the server identity comes out of `_meta`.
    #[test]
    fn t5_the_sdk_needs_ttl_ms_and_accepts_the_full_result() {
        assert!(serde_json::from_value::<DiscoverResult>(fixture(Some("ttlMs"))).is_err());
        let parsed = serde_json::from_value::<DiscoverResult>(fixture(None));
        assert!(parsed.is_ok(), "{parsed:?}");
        let info = parsed.ok().and_then(|result| result.server_info());
        assert_eq!(info.map(|info| info.name), Some("sylvode-mcp-server".to_string()));
    }

    /// Every other field the SDK requires is covered too: dropping any of them is a parse error.
    #[test]
    fn t5_every_required_field_matters() {
        for key in ["resultType", "supportedVersions", "capabilities", "cacheScope"] {
            assert!(
                serde_json::from_value::<DiscoverResult>(fixture(Some(key))).is_err(),
                "{key} is required"
            );
        }
    }

    /// A non-object extension value makes the result unparsable (the reason it must be an object).
    #[test]
    fn t5_a_non_object_extension_value_is_refused() {
        let mut result = fixture(None);
        result["capabilities"]["extensions"]["io.sylvode/task-protocol"] = json!(1);
        assert!(serde_json::from_value::<DiscoverResult>(result).is_err());
    }
}
