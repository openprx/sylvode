#![allow(clippy::indexing_slicing, clippy::type_complexity, clippy::needless_pass_by_value)]

use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::checks::{BotCreds, Channel, Context, Verdict, check_delivery, valid_project_key};
use super::paths::AllowedRoots;
use super::registry::SessionRegistry;
use super::sv_client::SvClient;
use crate::config::Config;

const BOT: &str = "00000000-0000-4000-8000-000000000001";
const TASK: &str = "11111111-1111-4111-8111-111111111111";
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const TOKEN: &str = concat!("opr", "_", "abababababababababababababababababababababababab");

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        fs::create_dir_all(std::env::temp_dir().join("sylvode-checks-home")).expect("home");
        let dir = std::env::temp_dir().join(format!("sylvode-checks-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        Self(dir.canonicalize().expect("canonical"))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn sub(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("mkdir");
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the mock Sylvode answers to the task view request.
#[derive(Clone)]
struct Mock {
    status: u16,
    body: Value,
    delay: Duration,
    seen: Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>>,
}

async fn view(
    State(mock): State<Mock>,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> (StatusCode, Json<Value>) {
    mock.seen.lock().push((
        uri.to_string(),
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        headers
            .get("x-sylvode-mcp-surface")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    ));
    tokio::time::sleep(mock.delay).await;
    (StatusCode::from_u16(mock.status).expect("status"), Json(mock.body))
}

async fn serve(mock: Mock) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = Router::new().route("/api/v1/ai/tasks/{id}", get(view)).with_state(mock);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, handle)
}

fn task_view(dir: &Path, channel: &str, cli: &str) -> Value {
    json!({
        "task_id": TASK,
        "attempt": 1,
        "active": true,
        "source": {"kind": "issue_assigned", "issue": {"id": "33333333-3333-4333-8333-333333333333"}},
        "project": {"id": PROJECT, "key": "SVWL", "name": "n", "workspace_id": PROJECT},
        "execution": {
            "enabled": true, "channel": channel, "cli": cli,
            "working_directory": dir.to_string_lossy(), "approval_gate": "none", "version": 1
        },
    })
}

fn delivery() -> Value {
    json!({
        "task_id": TASK, "project_id": PROJECT, "ai_participant_id": BOT, "attempts": 1, "max_attempts": 3,
        "project_key": "SVWL", "protocol": 1, "trigger_kind": "assigned"
    })
}

struct Fixture {
    base: Dir,
    root: PathBuf,
    project_dir: PathBuf,
    claude: PathBuf,
    registry: SessionRegistry,
}

impl Fixture {
    fn new() -> Self {
        let base = Dir::new();
        let root = base.sub("work");
        let project_dir = base.sub("work/svwl");
        let mcp = base.path().join("mcp-server");
        fs::write(&mcp, "#!/bin/sh\n").expect("mcp-server");
        fs::set_permissions(&mcp, fs::Permissions::from_mode(0o755)).expect("chmod");
        let claude = base.path().join("claude");
        fs::write(&claude, "#!/bin/sh\n").expect("claude");
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).expect("chmod");
        let registry = SessionRegistry::open(Some(&base.sub("state"))).expect("registry");
        Self {
            base,
            root,
            project_dir,
            claude,
            registry,
        }
    }

    fn config(&self, api: SocketAddr) -> Config {
        self.config_with(api, "")
    }

    fn config_with(&self, api: SocketAddr, runner_extra: &str) -> Config {
        Config::from_toml(&format!(
            "[server]\nlisten = \"127.0.0.1:0\"\n[security]\nallow_unsigned = true\n\
             [ai_task_callback]\nenabled = true\napi_url = \"http://{api}\"\n\
             [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"{TOKEN}\"\nsurface = \"mcp_stdio\"\n\
             [runner]\nallowed_roots = [\"{}\"]\ndelivery_check_timeout_secs = 1\n{runner_extra}\n\
             [runner.mcp]\nserver_binary = \"{}\"\n\
             [runner.cli.claude]\nbinary = \"{}\"\n\
             [[agents]]\nid = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"runner\"\n\
             [agents.route]\nbot_ids = [\"{BOT}\"]\n",
            self.root.display(),
            self.base.path().join("mcp-server").display(),
            self.claude.display()
        ))
        .expect("config")
    }

    async fn run(&self, config: &Config, mock: Mock, addr: SocketAddr, payload: &Value) -> Verdict {
        self.run_with(config, mock, addr, payload, None).await
    }

    async fn run_with(
        &self,
        config: &Config,
        _mock: Mock,
        addr: SocketAddr,
        payload: &Value,
        mismatch: Option<String>,
    ) -> Verdict {
        let roots = AllowedRoots::from_entries(
            &config.runner.allowed_roots,
            Some(&std::env::temp_dir().join("sylvode-checks-home")),
        )
        .map(|r| r.roots);
        let client = SvClient::new(reqwest::Client::new(), &format!("http://{addr}"), TOKEN, "mcp_stdio");
        check_delivery(
            Context {
                config,
                roots: &roots,
                registry: &self.registry,
                protocol_mismatch: mismatch,
                client,
                bot: bot(),
            },
            payload,
        )
        .await
    }
}

fn bot() -> BotCreds {
    BotCreds {
        bot_id: BOT.to_string(),
        api_url: "http://127.0.0.1:1".to_string(),
        workspace_id: "11111111-1111-4111-8111-111111111111".to_string(),
        token: TOKEN.to_string(),
    }
}

fn mock(status: u16, body: Value) -> Mock {
    Mock {
        status,
        body,
        delay: Duration::ZERO,
        seen: Arc::new(Mutex::new(Vec::new())),
    }
}

fn ok_mock(view: Value) -> Mock {
    mock(200, json!({"code": 0, "message": "success", "data": view}))
}

fn refused(verdict: &Verdict) -> (&'static str, String) {
    match verdict {
        Verdict::Refused(r) => (r.code, r.reason.clone()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_good_headless_delivery_runs_in_the_canonical_directory() {
    let fx = Fixture::new();
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    let verdict = fx.run(&config, m.clone(), addr, &delivery()).await;
    let Verdict::Run(accepted) = verdict else {
        panic!("{verdict:?}")
    };
    assert_eq!(accepted.dir, fx.project_dir);
    assert_eq!(accepted.cli, "claude");
    assert_eq!(accepted.channel, Channel::Headless);
    assert_eq!(accepted.program, fx.claude);
    assert_eq!(accepted.project_key, "SVWL");
    // The view was fetched with the bot's token and the transport header, for this attempt.
    let seen = m.seen.lock().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, format!("/api/v1/ai/tasks/{TASK}?attempt=1"));
    assert_eq!(seen[0].1.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(seen[0].2.as_deref(), Some("mcp_stdio"));
}

#[tokio::test]
async fn the_protocol_field_is_required() {
    let fx = Fixture::new();
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    for value in [
        None,
        Some(json!(0)),
        Some(json!(2)),
        Some(json!("1")),
        Some(json!(null)),
    ] {
        let mut payload = delivery();
        match &value {
            Some(v) => payload["protocol"] = v.clone(),
            None => {
                payload.as_object_mut().expect("object").remove("protocol");
            }
        }
        let (code, reason) = refused(&fx.run(&config, m.clone(), addr, &payload).await);
        assert_eq!(code, "protocol_mismatch", "{value:?}");
        assert!(reason.contains("runner 1"), "{reason}");
    }
    assert!(
        m.seen.lock().is_empty(),
        "Sylvode was asked before the protocol was checked"
    );
    let verdict = fx
        .run_with(
            &config,
            m.clone(),
            addr,
            &delivery(),
            Some("task protocol mismatch (runner 1, api 2)".into()),
        )
        .await;
    assert_eq!(refused(&verdict).0, "protocol_mismatch");
}

#[tokio::test]
async fn invalid_roots_refuse_the_runner_agents() {
    let fx = Fixture::new();
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    let roots = Err("runner.allowed_roots entry `/`: must not be `/`".to_string());
    let client = SvClient::new(reqwest::Client::new(), &format!("http://{addr}"), TOKEN, "mcp_stdio");
    let verdict = check_delivery(
        Context {
            config: &config,
            roots: &roots,
            registry: &fx.registry,
            protocol_mismatch: None,
            client,
            bot: bot(),
        },
        &delivery(),
    )
    .await;
    let (code, reason) = refused(&verdict);
    assert_eq!(code, "runner_agent_disabled");
    assert!(!reason.contains('/'), "{reason}");
    assert!(m.seen.lock().is_empty());
}

#[tokio::test]
async fn sylvode_answers_decide_between_ignored_unavailable_and_refused() {
    let fx = Fixture::new();
    let ok = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(ok.clone()).await;
    let config = fx.config(addr);
    let cases: [(u16, &str); 6] = [
        (400, "ignored"),
        (500, "unavailable"),
        (503, "unavailable"),
        (401, "runner_agent_disabled"),
        (403, "runner_agent_disabled"),
        (404, "delivery_mismatch"),
    ];
    for (status, want) in cases {
        let m = mock(status, json!({"code": status, "message": "x"}));
        let (addr, _server) = serve(m.clone()).await;
        let config = fx.config(addr);
        let verdict = fx.run(&config, m, addr, &delivery()).await;
        match (&verdict, want) {
            (Verdict::Ignored(reason), "ignored") => {
                assert_eq!(reason, "attempt 1 is no longer active in Sylvode");
            }
            (Verdict::Unavailable, "unavailable") => {}
            (Verdict::Refused(r), code) if r.code == code => {}
            _ => panic!("status {status}: {verdict:?}"),
        }
    }
    // A view that says the task is not active is ignored too.
    let mut inactive = task_view(&fx.project_dir, "headless", "claude");
    inactive["active"] = json!(false);
    let m = ok_mock(inactive);
    let (addr2, _server2) = serve(m.clone()).await;
    let verdict = fx.run(&fx.config(addr2), m, addr2, &delivery()).await;
    assert!(matches!(verdict, Verdict::Ignored(_)), "{verdict:?}");
    drop(config);
}

#[tokio::test]
async fn a_slow_sylvode_is_unavailable_not_refused() {
    let fx = Fixture::new();
    let mut m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    m.delay = Duration::from_secs(3);
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    let started = std::time::Instant::now();
    let verdict = fx.run(&config, m, addr, &delivery()).await;
    assert!(matches!(verdict, Verdict::Unavailable), "{verdict:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
}

#[tokio::test]
async fn an_unreachable_sylvode_is_unavailable() {
    let fx = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let config = fx.config(addr);
    let verdict = fx.run(&config, mock(200, json!({})), addr, &delivery()).await;
    assert!(matches!(verdict, Verdict::Unavailable), "{verdict:?}");
}

#[tokio::test]
async fn the_cross_check_refuses_a_delivery_that_differs_from_sylvodes_record() {
    let fx = Fixture::new();
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    for (field, value) in [
        ("project_id", "99999999-9999-4999-8999-999999999999"),
        ("project_key", "OTHER"),
        ("task_id", "99999999-9999-4999-8999-999999999999"),
    ] {
        let mut payload = delivery();
        payload[field] = json!(value);
        let (code, _) = refused(&fx.run(&config, m.clone(), addr, &payload).await);
        assert_eq!(code, "delivery_mismatch", "{field}");
    }
    for bad in ["not-a-uuid", ""] {
        let mut payload = delivery();
        payload["task_id"] = json!(bad);
        assert_eq!(
            refused(&fx.run(&config, m.clone(), addr, &payload).await).0,
            "delivery_mismatch"
        );
    }
}

#[tokio::test]
async fn execution_settings_gate_the_run() {
    let fx = Fixture::new();
    for (mutate, want) in [
        (json!({"execution": null}), "execution_disabled"),
        (
            json!({"execution": {"enabled": false, "channel": "headless", "cli": "claude", "working_directory": "/x"}}),
            "execution_disabled",
        ),
    ] {
        let mut v = task_view(&fx.project_dir, "headless", "claude");
        v["execution"] = mutate["execution"].clone();
        let m = ok_mock(v);
        let (addr, _server) = serve(m.clone()).await;
        let config = fx.config(addr);
        assert_eq!(refused(&fx.run(&config, m, addr, &delivery()).await).0, want);
    }
}

#[tokio::test]
async fn project_keys_are_session_names_bound_to_one_project() {
    for (key, valid) in [
        ("SVWL", true),
        ("A", true),
        ("123", true),
        ("", false),
        ("svwl", false),
        ("SV-WL", false),
        ("SV:WL", false),
    ] {
        assert_eq!(valid_project_key(key), valid, "{key:?}");
    }
    assert!(valid_project_key(&"K".repeat(32)) && !valid_project_key(&"K".repeat(33)));

    let fx = Fixture::new();
    // An invalid key from Sylvode is refused.
    let mut v = task_view(&fx.project_dir, "headless", "claude");
    v["project"]["key"] = json!("svwl");
    let m = ok_mock(v);
    let (addr, _server) = serve(m.clone()).await;
    let mut payload = delivery();
    payload["project_key"] = json!("svwl");
    assert_eq!(
        refused(&fx.run(&fx.config(addr), m, addr, &payload).await).0,
        "invalid_project_key"
    );

    // A second project using the key of the first collides.
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    assert!(matches!(
        fx.run(&config, m.clone(), addr, &delivery()).await,
        Verdict::Run(_)
    ));
    let mut other_view = task_view(&fx.project_dir, "headless", "claude");
    let other_project = "44444444-4444-4444-8444-444444444444";
    other_view["project"]["id"] = json!(other_project);
    let m2 = ok_mock(other_view);
    let (addr2, _server2) = serve(m2.clone()).await;
    let mut payload = delivery();
    payload["project_id"] = json!(other_project);
    assert_eq!(
        refused(&fx.run(&fx.config(addr2), m2, addr2, &payload).await).0,
        "session_name_collision"
    );
    // The first project still works.
    assert!(matches!(fx.run(&config, m, addr, &delivery()).await, Verdict::Run(_)));
}

#[tokio::test]
async fn working_directories_are_checked_against_the_roots() {
    let fx = Fixture::new();
    let outside = fx.base.sub("elsewhere");
    let missing = fx.root.join("absent");
    let file = fx.root.join("a-file");
    fs::write(&file, "x").expect("write");
    for (dir, want) in [
        (outside.clone(), "path_outside_allowlist"),
        (missing, "path_missing"),
        (file, "path_not_directory"),
        (fx.root.join("svwl/../../elsewhere"), "path_outside_allowlist"),
    ] {
        let m = ok_mock(task_view(&dir, "headless", "claude"));
        let (addr, _server) = serve(m.clone()).await;
        let config = fx.config(addr);
        let (code, reason) = refused(&fx.run(&config, m, addr, &delivery()).await);
        assert_eq!(code, want, "{dir:?}");
        // The reason never carries a path.
        assert!(
            !reason.contains(&*fx.base.path().to_string_lossy()) && !reason.contains('/'),
            "{reason}"
        );
    }
}

#[tokio::test]
async fn channel_and_cli_prerequisites() {
    let fx = Fixture::new();
    // The tmux channel needs tmux (at the minimum version) and the MCP server on this runner.
    let m = ok_mock(task_view(&fx.project_dir, "tmux", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let fake = |name: &str, body: &str| {
        let path = fx.base.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    };
    let mut config = fx.config(addr);
    config.runner.tmux.enabled = true;
    config.runner.tmux.binary = fx.base.path().join("no-such-tmux").to_string_lossy().into_owned();
    assert_eq!(
        refused(&fx.run(&config, m.clone(), addr, &delivery()).await).0,
        "tmux_unavailable"
    );
    config.runner.tmux.binary = fake("old-tmux", "echo 'tmux 3.1'").to_string_lossy().into_owned();
    let (code, reason) = refused(&fx.run(&config, m.clone(), addr, &delivery()).await);
    assert_eq!(
        (code, reason.as_str()),
        ("tmux_unavailable", "tmux 3.1 is older than 3.2")
    );
    config.runner.tmux.binary = fake("junk-tmux", "echo 'not tmux'").to_string_lossy().into_owned();
    assert_eq!(
        refused(&fx.run(&config, m.clone(), addr, &delivery()).await).0,
        "tmux_unavailable"
    );
    config.runner.tmux.binary = fake("tmux", "echo 'tmux 3.7b'").to_string_lossy().into_owned();
    config.runner.mcp.server_binary = fx.base.path().join("no-such-mcp").to_string_lossy().into_owned();
    let (code, reason) = refused(&fx.run(&config, m.clone(), addr, &delivery()).await);
    assert_eq!(code, "cli_unavailable");
    assert!(reason.contains("MCP server"), "{reason}");
    config.runner.mcp.server_binary = fake("mcp-server-2", "exit 0").to_string_lossy().into_owned();
    let verdict = fx.run(&config, m.clone(), addr, &delivery()).await;
    let Verdict::Run(accepted) = verdict else {
        panic!("{verdict:?}")
    };
    assert_eq!(accepted.channel, Channel::Tmux);
    assert!(
        accepted.message.starts_with("[Sylvode task] id="),
        "{}",
        accepted.message
    );
    assert_eq!(accepted.project_id, PROJECT);
    // codex and opencode are off by default.
    for cli in ["codex", "opencode"] {
        let m = ok_mock(task_view(&fx.project_dir, "headless", cli));
        let (addr, _server) = serve(m.clone()).await;
        assert_eq!(
            refused(&fx.run(&fx.config(addr), m, addr, &delivery()).await).0,
            "cli_disabled",
            "{cli}"
        );
    }
    // Enabled but not installed.
    let m = ok_mock(task_view(&fx.project_dir, "headless", "codex"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config_with(addr, "");
    let mut enabled = config.clone();
    enabled.runner.cli.codex.enabled = true;
    enabled.runner.cli.codex.binary = fx.base.path().join("no-such-codex").to_string_lossy().into_owned();
    assert_eq!(
        refused(&fx.run(&enabled, m.clone(), addr, &delivery()).await).0,
        "cli_unavailable"
    );
    // claude not installed.
    let mut no_claude = config.clone();
    no_claude.runner.cli.claude.binary = fx.base.path().join("no-such-claude").to_string_lossy().into_owned();
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    assert_eq!(
        refused(&fx.run(&no_claude, m, addr, &delivery()).await).0,
        "cli_unavailable"
    );
    // An unknown channel or CLI.
    for (channel, cli) in [("carrier-pigeon", "claude"), ("headless", "emacs")] {
        let m = ok_mock(task_view(&fx.project_dir, channel, cli));
        let (addr, _server) = serve(m.clone()).await;
        let code = refused(&fx.run(&config, m, addr, &delivery()).await).0;
        assert!(matches!(code, "delivery_mismatch" | "cli_unavailable"), "{code}");
    }
}

#[tokio::test]
async fn no_refusal_text_holds_the_token() {
    let fx = Fixture::new();
    let mut texts = Vec::new();
    for (status, channel) in [(401, "headless"), (404, "headless"), (200, "carrier-pigeon")] {
        let body = if status == 200 {
            json!({"code": 0, "data": task_view(&fx.project_dir, channel, "claude")})
        } else {
            json!({"code": status, "message": format!("token {TOKEN} refused")})
        };
        let m = mock(status, body);
        let (addr, _server) = serve(m.clone()).await;
        if let Verdict::Refused(r) = fx.run(&fx.config(addr), m, addr, &delivery()).await {
            texts.push(r.reason);
        }
    }
    assert_eq!(texts.len(), 3);
    assert!(
        texts.iter().all(|t| !t.contains("opr_") && !t.contains(TOKEN)),
        "{texts:?}"
    );
}

#[tokio::test]
async fn a_refused_delivery_never_binds_a_key() {
    let fx = Fixture::new();
    let outside = fx.base.sub("elsewhere");
    // Refused at the path check (and at the channel check): the key stays free.
    for (dir, channel, cli) in [
        (outside.clone(), "headless", "claude"),
        (fx.project_dir.clone(), "carrier-pigeon", "claude"),
        (fx.project_dir.clone(), "headless", "emacs"),
        (fx.project_dir.clone(), "headless", "codex"),
    ] {
        let m = ok_mock(task_view(&dir, channel, cli));
        let (addr, _server) = serve(m.clone()).await;
        let verdict = fx.run(&fx.config(addr), m, addr, &delivery()).await;
        assert!(matches!(verdict, Verdict::Refused(_)), "{verdict:?}");
        assert!(
            fx.registry.entries().is_empty(),
            "{channel} {cli}: a refused delivery bound a key"
        );
    }
    // The legitimate project, delivered after a squatter was refused, is not in collision.
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    assert!(matches!(
        fx.run(&fx.config(addr), m, addr, &delivery()).await,
        Verdict::Run(_)
    ));
    assert_eq!(fx.registry.owner("SVWL").as_deref(), Some(PROJECT));
}

/// The tmux channel is an operator opt-in: off, a project that asks for it is refused with a stable
/// code while headless keeps working; codex and opencode are refused on tmux whatever the switch.
#[tokio::test]
async fn the_tmux_channel_needs_the_operator_switch_and_only_claude_is_verified() {
    let fx = Fixture::new();
    let m = ok_mock(task_view(&fx.project_dir, "tmux", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    let config = fx.config(addr);
    assert!(!config.runner.tmux.enabled, "off by default");
    let (code, reason) = refused(&fx.run(&config, m, addr, &delivery()).await);
    assert_eq!(code, "tmux_channel_disabled");
    assert!(reason.contains("[runner.tmux] enabled = true"), "{reason}");
    // The headless channel is unaffected by the switch.
    let m = ok_mock(task_view(&fx.project_dir, "headless", "claude"));
    let (addr, _server) = serve(m.clone()).await;
    assert!(matches!(
        fx.run(&fx.config(addr), m, addr, &delivery()).await,
        Verdict::Run(_)
    ));
    // codex and opencode on tmux: refused entirely, enabled or not.
    for cli in ["codex", "opencode"] {
        for switch in [false, true] {
            let m = ok_mock(task_view(&fx.project_dir, "tmux", cli));
            let (addr, _server) = serve(m.clone()).await;
            let mut config = fx.config(addr);
            config.runner.tmux.enabled = switch;
            config.runner.cli.codex.enabled = true;
            config.runner.cli.opencode.enabled = true;
            let (code, _) = refused(&fx.run(&config, m, addr, &delivery()).await);
            assert_eq!(code, "cli_unverified", "{cli} switch={switch}");
        }
    }
}
