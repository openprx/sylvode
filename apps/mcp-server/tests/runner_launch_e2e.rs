//! The runner -> CLI -> MCP chain, with the real `mcp-server` binary.
//!
//! The runner writes one MCP configuration file per bot and hands the CLI a registration that
//! says "run `mcp-server serve --transport stdio --config <file>`". Nothing else tells the server
//! who it speaks for, so a file the server cannot start from means a CLI with no MCP server at all
//! (the server refused to start for want of a workspace id, and no test noticed: the runner's tests
//! use a fake CLI and this crate's tests always wrote a workspace id themselves).
//!
//! Here nothing is typed by hand on the runner's side. The configuration text is the golden block
//! Sylvode issues; the runner's own loader reads it; its own writer produces the per-bot file and
//! the CLI registration; its own launch functions produce the CLI argument vectors; and this test
//! starts the shipped binary from exactly the command and arguments those produced. It then speaks
//! MCP over stdio against an in-process stand-in for the API that behaves like the API for a runner
//! bot: the bot's token and the `mcp_stdio` surface are required, and every route the server calls
//! must be one the API's route-scope table lets a runner bot call without naming a project.

#![cfg(unix)]
#![allow(clippy::indexing_slicing)]

use axum::{
    Json, Router,
    body::Bytes,
    extract::{MatchedPath, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use sylvode_runner::mcp_launch_for_tests::{
    AiTaskCallback, BotCreds, Config, LaunchFiles, SEARCH_MODE_TOOLS, SERVER_NAME, SessionName, StateDir, WRITE_TOOL,
    claude_headless_argv, codex_headless_argv, session_argv,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

type TestResult = Result<(), Box<dyn Error>>;

/// The block Sylvode issues: the runner's loader reads the very bytes the API test pins.
const GOLDEN_BLOCK: &str = include_str!("../../api/tests/fixtures/runner-config-block.toml");
const GOLDEN_BOT: &str = "00000000-0000-0000-0000-000000000000";
const GOLDEN_WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const GOLDEN_TOKEN: &str = "opr_golden_fixture_token";
const GOLDEN_API_URL: &str = "api_url = \"https://sylvode.example.net\"";
const TOKEN: &str = concat!("opr", "_", "runnerlaunche2etokenabcdef0123456789");
const TASK: &str = "22222222-2222-4222-8222-222222222222";
const SERVER: &str = env!("CARGO_BIN_EXE_mcp-server");

// ---- the stand-in API ----

#[derive(Clone, Debug)]
struct Call {
    method: String,
    pattern: String,
    authorized: bool,
    surface: Option<String>,
}

type Log = Arc<Mutex<Vec<Call>>>;

async fn answer(
    State(log): State<Log>,
    path: MatchedPath,
    method: Method,
    headers: HeaderMap,
    _body: Bytes,
) -> Response {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let authorized = header("authorization").as_deref() == Some(&format!("Bearer {TOKEN}"));
    let surface = header("x-sylvode-mcp-surface");
    log.lock().push(Call {
        method: method.to_string(),
        pattern: path.as_str().to_string(),
        authorized,
        surface: surface.clone(),
    });
    // The API refuses a token presented for another transport than the one it was registered for.
    if !authorized || surface.as_deref() != Some("mcp_stdio") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code": 401, "message": "bot credential is not valid for the presented transport"})),
        )
            .into_response();
    }
    let data = match path.as_str() {
        "/api/v1/ai/tasks/{task_id}" => json!({
            "id": TASK,
            "status": "processing",
            "attempts": 1,
            "project": {"id": "33333333-3333-4333-8333-333333333333", "key": "SVWL"},
        }),
        "/api/v1/projects/{project_id}/agent-policy" => json!({"mcp": {}}),
        "/api/v1/ai/tasks/{task_id}/ack" => json!({"proceed": true, "status": "processing"}),
        _ => json!({"status": "completed"}),
    };
    Json(json!({"code": 0, "message": "ok", "data": data})).into_response()
}

async fn spawn_api() -> Result<(String, Log), Box<dyn Error>> {
    let log: Log = Arc::default();
    let router = Router::new()
        .route("/api/v1/ai/tasks/{task_id}", get(answer))
        .route("/api/v1/projects/{project_id}/agent-policy", get(answer))
        .route("/api/v1/ai/tasks/{task_id}/ack", post(answer))
        .route("/api/v1/ai/tasks/{task_id}/complete", post(answer))
        .with_state(Arc::clone(&log));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let url = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((url, log))
}

// ---- the runner's side ----

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, Box<dyn Error>> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sylvode-runner-launch-e2e-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("project"))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the CLI is given for MCP, as the runner produced it.
struct Registration {
    command: String,
    args: Vec<String>,
}

/// The registration in codex's `-c mcp_servers.<name>.command=...` and `.args=...` overrides.
fn codex_registration(words: &[String]) -> Result<Registration, Box<dyn Error>> {
    let mut command = None;
    let mut args = None;
    for pair in words.windows(2).filter(|pair| pair[0] == "-c") {
        let table: toml::Table = toml::from_str(&pair[1])?;
        let Some(entry) = table.get("mcp_servers").and_then(|v| v.get(SERVER_NAME)) else {
            continue;
        };
        if let Some(text) = entry.get("command").and_then(toml::Value::as_str) {
            command = Some(text.to_string());
        }
        if let Some(list) = entry.get("args").and_then(toml::Value::as_array) {
            args = Some(list.iter().filter_map(|v| v.as_str().map(String::from)).collect());
        }
    }
    Ok(Registration {
        command: command.ok_or("codex was given no MCP command")?,
        args: args.ok_or("codex was given no MCP args")?,
    })
}

/// Everything the runner hands to one CLI launch.
struct Launch {
    /// Every argument vector the runner builds for the CLI (interactive and headless).
    cli_argvs: Vec<Vec<String>>,
    /// One registration per argument vector or file; they must all name the same server command.
    registrations: Vec<Registration>,
    mcp_toml: PathBuf,
    mcp_toml_text: String,
}

/// Loads the golden block with the stand-in API's address and a usable token, as the runner's own
/// loader does at start-up.
fn runner_config(api_url: &str, extra: &str) -> Result<Config, Box<dyn Error>> {
    let text = GOLDEN_BLOCK
        .replace(GOLDEN_API_URL, &format!("api_url = \"{api_url}\""))
        .replace(&format!("token = \"{GOLDEN_TOKEN}\""), &format!("token = \"{TOKEN}\""));
    assert!(text.contains(TOKEN) && text.contains(api_url), "the block was adapted");
    Ok(Config::from_toml(&format!("{text}\n{extra}"))?)
}

/// What the runner does for one delivery of `cli`: resolves the bot, writes its files, builds the
/// CLI's argument vectors.
fn runner_launch(scratch: &Scratch, config: &Config, cli: &'static str) -> Result<Launch, Box<dyn Error>> {
    let AiTaskCallback::Report(target) = config.ai_task_callback_for(GOLDEN_BOT) else {
        return Err("the runner has no usable credentials for the golden bot".into());
    };
    let bot = BotCreds::for_target(GOLDEN_BOT, &target).ok_or("the target has no workspace")?;
    let state = StateDir::open(&scratch.0.join("state"))?;
    let server = Path::new(SERVER);
    state.write_bot_files(&bot.mcp_files(&config.runner), server)?;

    let name = SessionName::new("SVWL", "claude")?;
    let mcp_toml = state.mcp_toml(GOLDEN_BOT);
    let claude_json = state.claude_mcp_json(GOLDEN_BOT);
    let settings = state.claude_settings(&name);
    let socket = state.hook_socket();
    let runner_exe = scratch.0.join("sylvode-runner");
    let files = LaunchFiles {
        claude_mcp_json: &claude_json,
        claude_settings: &settings,
        runner_exe: &runner_exe,
        hook_socket: &socket,
    };
    let program = scratch.0.join(cli);
    let mut cli_argvs = Vec::new();
    let mut registrations = Vec::new();
    match cli {
        "claude" => {
            // A session the runner creates and a headless run: both are given the runner's MCP file.
            cli_argvs.push(session_argv(&program, &config.runner.cli.claude, &files)?);
            cli_argvs.push(claude_headless_argv(
                &program,
                &config.runner.cli.claude,
                &claude_json,
                config.runner.mcp.allow_write_call,
                "[Sylvode task] id=x",
            )?);
            // The CLI reads the registration from the file `--mcp-config` names.
            for argv in &cli_argvs {
                let at = argv.iter().position(|a| a == "--mcp-config").ok_or("no --mcp-config")?;
                assert_eq!(argv.get(at + 1).map(String::as_str), claude_json.to_str());
            }
            let registration: Value = serde_json::from_str(&std::fs::read_to_string(&claude_json)?)?;
            let entry = &registration["mcpServers"][SERVER_NAME];
            assert_eq!(entry["type"], "stdio");
            registrations.push(Registration {
                command: entry["command"].as_str().ok_or("no command")?.to_string(),
                args: serde_json::from_value(entry["args"].clone())?,
            });
        }
        "codex" => {
            cli_argvs.push(codex_headless_argv(
                &program,
                &config.runner.cli.codex,
                server,
                &mcp_toml,
                "[Sylvode task] id=x",
            )?);
            // codex takes the registration as `-c mcp_servers.<name>.command=...` / `.args=...`.
            for argv in &cli_argvs {
                registrations.push(codex_registration(argv)?);
            }
        }
        other => return Err(format!("unexpected cli {other}").into()),
    }
    Ok(Launch {
        cli_argvs,
        registrations,
        mcp_toml_text: std::fs::read_to_string(&mcp_toml)?,
        mcp_toml,
    })
}

// ---- the CLI's side: an MCP client over stdio ----

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    /// Starts the server the way the CLI does: the registered command with the registered
    /// arguments, in the project directory, with nothing but a bare environment.
    fn start(registration: &Registration, dir: &Path) -> Result<Self, Box<dyn Error>> {
        let mut child = Command::new(&registration.command)
            .args(&registration.args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().ok_or("stdin was not piped")?;
        let stdout = child.stdout.take().ok_or("stdout was not piped")?;
        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        })
    }

    async fn send(&mut self, message: &Value) -> TestResult {
        self.stdin.write_all(format!("{message}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(Duration::from_secs(20), self.reader.read_line(&mut line)).await??;
            if read == 0 {
                let status = self.child.try_wait()?;
                return Err(format!("the MCP server closed its output before answering {method} ({status:?})").into());
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == json!(id) {
                return Ok(message);
            }
        }
    }

    async fn initialize(&mut self) -> Result<Value, Box<dyn Error>> {
        let answer = self
            .request(
                "initialize",
                json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}),
            )
            .await?;
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        Ok(answer)
    }

    async fn tool_names(&mut self) -> Result<Vec<String>, Box<dyn Error>> {
        let answer = self.request("tools/list", json!({})).await?;
        let tools = answer["result"]["tools"]
            .as_array()
            .ok_or_else(|| format!("tools/list failed: {answer}"))?;
        Ok(tools
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(String::from))
            .collect())
    }

    async fn call(&mut self, name: &str, arguments: Value) -> Result<Value, Box<dyn Error>> {
        let answer = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        let result = answer.get("result").ok_or_else(|| format!("{name} failed: {answer}"))?;
        if result["isError"] == json!(true) {
            return Err(format!("{name} returned an error: {result}").into());
        }
        Ok(result.clone())
    }

    async fn finish(mut self) -> TestResult {
        drop(self.stdin);
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

/// One launch, from the loaded block to a task answered through MCP.
async fn chain(cli: &'static str, extra_config: &str, allow_write: bool, full: bool) -> TestResult {
    let scratch = Scratch::new()?;
    let (api_url, log) = spawn_api().await?;
    let config = runner_config(&api_url, extra_config)?;
    let launch = runner_launch(&scratch, &config, cli)?;

    // The per-bot file: the workspace the block named, the token only here, the mode asked for.
    let file: toml::Table = toml::from_str(&launch.mcp_toml_text)?;
    let mcp = file["mcp"].as_table().ok_or("no [mcp]")?;
    assert_eq!(mcp["workspace_id"].as_str(), Some(GOLDEN_WORKSPACE));
    assert_eq!(mcp["bot_token"].as_str(), Some(TOKEN));
    assert_eq!(mcp["api_url"].as_str(), Some(api_url.as_str()));
    assert_eq!(mcp["tool_mode"].as_str(), Some(if full { "full" } else { "search" }));
    assert_eq!(mcp["allow_write_call"].as_bool(), Some(allow_write));

    // No secret on any command line: the CLI's, or the one it runs for the server.
    for argv in launch
        .cli_argvs
        .iter()
        .chain(launch.registrations.iter().map(|r| &r.args))
    {
        assert!(
            argv.iter().all(|word| !word.contains(TOKEN) && !word.contains("opr_")),
            "a token is on a command line: {argv:?}"
        );
    }

    for registration in &launch.registrations {
        assert_eq!(registration.command, SERVER);
        assert_eq!(
            registration.args,
            [
                "serve",
                "--transport",
                "stdio",
                "--config",
                launch.mcp_toml.to_str().ok_or("path")?
            ]
        );
        log.lock().clear();

        let mut client = McpClient::start(registration, &scratch.0.join("project"))?;
        let initialized = client.initialize().await?;
        assert!(
            initialized["result"]["capabilities"]["tools"].is_object(),
            "{initialized}"
        );

        let names = client.tool_names().await?;
        // `search` lists the entry tools beside the task tools (the runner's allow list names
        // both, and `sylvode_write` is listed whether or not the file allows calling it); `full`
        // lists the registry, which has no entry tools but still the task tools.
        let needed: Vec<&str> = SEARCH_MODE_TOOLS
            .iter()
            .copied()
            .chain([WRITE_TOOL])
            .filter(|tool| !full || tool.starts_with("sylvode_task_"))
            .collect();
        for tool in needed {
            assert!(
                names.iter().any(|n| n == tool),
                "{cli}: {tool} is not listed: {names:?}"
            );
        }
        assert_eq!(
            names.len() > SEARCH_MODE_TOOLS.len() + 1,
            full,
            "{cli}: {} tools",
            names.len()
        );

        let task = client
            .call("sylvode_task_get", json!({"task_id": TASK, "attempt": 1}))
            .await?;
        assert!(task.to_string().contains(TASK), "the task is returned: {task}");
        client
            .call(
                "sylvode_task_ack",
                json!({"task_id": TASK, "attempt": 1, "plan": "Do it."}),
            )
            .await?;
        client
            .call(
                "sylvode_task_complete",
                json!({"task_id": TASK, "attempt": 1, "outcome": "done", "summary": "Done."}),
            )
            .await?;
        client.finish().await?;

        let calls = log.lock().clone();
        assert_eq!(
            calls.first().map(|c| format!("{} {}", c.method, c.pattern)).as_deref(),
            Some("GET /api/v1/ai/tasks/{task_id}"),
            "{calls:?}"
        );
        for call in &calls {
            assert!(call.authorized, "the bot's token is presented: {call:?}");
            assert_eq!(call.surface.as_deref(), Some("mcp_stdio"), "{call:?}");
            // Whatever the server calls must be open to a runner bot: the task routes name no
            // project, and the project's agent policy is resolved to the task's own project.
            let route = api::middleware::runner_bot_scope::lookup(&call.method, &call.pattern)
                .ok_or_else(|| format!("{} {} is not classified for runner bots", call.method, call.pattern))?;
            let expected = if call.pattern.ends_with("/agent-policy") {
                "project-resolved"
            } else {
                "no project"
            };
            assert_eq!(route.class.name(), expected, "{call:?}");
        }
        for pattern in ["/api/v1/ai/tasks/{task_id}/ack", "/api/v1/ai/tasks/{task_id}/complete"] {
            assert!(
                calls.iter().any(|c| c.pattern == pattern),
                "{pattern} was called: {calls:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_claude_launch_reaches_its_task_through_the_real_mcp_server() -> TestResult {
    chain("claude", "", false, false).await
}

#[tokio::test]
async fn a_codex_launch_reaches_its_task_through_the_real_mcp_server() -> TestResult {
    // `[runner.mcp] allow_write_call` is one switch for every CLI's MCP file.
    chain("codex", "[runner.mcp]\nallow_write_call = true\n", true, false).await
}

#[tokio::test]
async fn the_full_tool_mode_of_the_runner_configuration_reaches_the_server() -> TestResult {
    let extra = "[runner.mcp]\ntool_mode = \"full\"\nallow_write_call = true\n";
    chain("claude", extra, true, true).await
}
