//! The tunnel through the real executable and an in-process control plane: every inbound
//! envelope goes through the production verification and gating in the tunnel loop.

use crate::support::{
    CANONICAL, ControlPlane, Envelope, SECRET, Server, StubApi, TempDir, TunnelPeer, path_with, wait_lines,
    write_script,
};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

const AGENT_ID: &str = "agent-1";
const TUNNEL_SECRET: &str = "tunnel-secret-for-tests";
const WAIT: Duration = Duration::from_secs(15);

struct Options<'a> {
    cli_enabled: bool,
    callback_enabled: bool,
    require_inbound_sig: bool,
    cli_max_concurrency: usize,
    extra_agents: &'a str,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            cli_enabled: true,
            callback_enabled: true,
            require_inbound_sig: false,
            cli_max_concurrency: 4,
            extra_agents: "",
        }
    }
}

fn config(plane: &ControlPlane, api: &StubApi, options: &Options<'_>) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [features]\ntunnel_enabled = true\ncli_enabled = {cli}\ncallback_enabled = {callback}\n\n\
         [runtime]\ncli_max_concurrency = {concurrency}\n\n\
         [tunnel]\nenabled = true\nurl = \"{url}\"\nagent_id = \"{AGENT_ID}\"\nauth_token = \"tunnel-token\"\n\
         reconnect_secs = 1\nheartbeat_secs = 30\nhmac_secret = \"{TUNNEL_SECRET}\"\nrequire_inbound_sig = {require}\n\n\
         {extra}\n\
         [[agents]]\nid = \"runner\"\nname = \"Runner\"\nagent_type = \"cli\"\n\n\
         [agents.cli]\nexecutor = \"codex\"\ntimeout_secs = 60\ncallback = \"api\"\ncallback_url = \"{callback_url}\"\n\
         callback_token = \"cb-token\"\nupdate_state_on_fail = \"todo\"\nupdate_state_on_success = \"done\"\n",
        cli = options.cli_enabled,
        callback = options.callback_enabled,
        concurrency = options.cli_max_concurrency,
        url = plane.url(),
        require = options.require_inbound_sig,
        extra = options.extra_agents,
        callback_url = api.url("/callback"),
    )
}

/// A fake `codex` that records each run in `runs` (`start`, a one-second pause unless `fast`,
/// then `end`).
fn codex(dir: &TempDir, fast: bool) -> PathBuf {
    let log = dir.file("runs");
    let pause = if fast { "" } else { "sleep 1\n" };
    write_script(
        dir.path(),
        "codex",
        &format!("echo start >> '{l}'\n{pause}echo end >> '{l}'", l = log.display()),
    );
    log
}

struct Harness {
    _server: Server,
    peer: TunnelPeer,
    api: StubApi,
    runs: PathBuf,
    _bin: TempDir,
}

async fn start(options: Options<'_>, fast: bool) -> Harness {
    let bin = TempDir::new("bin");
    let runs = codex(&bin, fast);
    let api = StubApi::start().await;
    let plane = ControlPlane::bind().await;
    let server = Server::start(
        CANONICAL,
        &config(&plane, &api, &options),
        &[("PATH", &path_with(bin.path()))],
    )
    .expect("server starts");
    let peer = plane.accept(WAIT).await;
    Harness {
        _server: server,
        peer,
        api,
        runs,
        _bin: bin,
    }
}

fn dispatch(issue: &str, agent: Option<&str>) -> Envelope {
    let mut payload = json!({"run_id": format!("run-{issue}"), "issue_id": issue, "body": {"data": {"issue": {"id": issue, "title": "tunnel task"}}}});
    if let Some(agent) = agent {
        payload["agent"] = json!(agent);
    }
    Envelope::new("task.dispatch", AGENT_ID, payload)
}

async fn next(peer: &mut TunnelPeer) -> Envelope {
    peer.recv(WAIT).await.expect("an envelope from the service")
}

/// Every envelope the service sends is signed with the shared secret.
fn assert_signed_by_service(envelope: &Envelope) {
    let resigned = envelope.clone().signed(TUNNEL_SECRET);
    assert_eq!(envelope.sig, resigned.sig, "outbound envelope signature: {envelope:?}");
}

/// The tunnel connects with the configured bearer token, refuses an envelope signed with another
/// secret (`bad_signature`, nothing dispatched), and dispatches a correctly signed one.
#[tokio::test(flavor = "multi_thread")]
async fn a_wrongly_signed_envelope_is_refused_and_a_signed_one_runs() {
    let mut h = start(Options::default(), true).await;
    assert_eq!(h.peer.authorization.as_deref(), Some("Bearer tunnel-token"));

    let forged = dispatch("ISSUE-FORGED", None).signed("not-the-tunnel-secret");
    h.peer.send(&forged).await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.msg_type, "error", "{refused:?}");
    assert_eq!(refused.payload["reason"], "bad_signature");
    assert_eq!(refused.payload["msg_id"], forged.id.as_str());
    assert_signed_by_service(&refused);

    h.peer.send(&dispatch("ISSUE-1", None).signed(TUNNEL_SECRET)).await;
    let ack = next(&mut h.peer).await;
    assert_eq!(ack.msg_type, "task.ack", "{ack:?}");
    assert_eq!(ack.payload["issue_id"], "ISSUE-1");
    assert_signed_by_service(&ack);
    let result = next(&mut h.peer).await;
    assert_eq!(result.msg_type, "task.result", "{result:?}");
    assert_eq!(result.payload["status"], "success", "{result:?}");
    assert_eq!(result.payload["issue_id"], "ISSUE-1");
    assert_eq!(
        wait_lines(&h.runs, 2, Duration::from_secs(1)),
        ["start", "end"],
        "only the signed task ran"
    );
}

/// With `require_inbound_sig = true` an envelope without `sig` is refused as
/// `missing_signature`; without it, the same envelope is refused as `bad_signature`.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_envelope_is_refused_with_the_configured_reason() {
    for (require, reason) in [(true, "missing_signature"), (false, "bad_signature")] {
        let mut h = start(
            Options {
                require_inbound_sig: require,
                ..Options::default()
            },
            true,
        )
        .await;
        let unsigned = dispatch("ISSUE-UNSIGNED", None);
        h.peer.send(&unsigned).await;
        let refused = next(&mut h.peer).await;
        assert_eq!(refused.msg_type, "error", "{refused:?}");
        assert_eq!(refused.payload["reason"], reason, "require_inbound_sig = {require}");
        assert_eq!(refused.payload["msg_id"], unsigned.id.as_str());
        assert!(
            h.peer.recv(Duration::from_millis(500)).await.is_none(),
            "nothing dispatched"
        );
        assert!(wait_lines(&h.runs, 1, Duration::from_millis(200)).is_empty());
    }
}

/// A tunnel task needs `[features].cli_enabled`; without it the executor never starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_task_does_not_run_without_cli_enabled() {
    let mut h = start(
        Options {
            cli_enabled: false,
            ..Options::default()
        },
        true,
    )
    .await;
    h.peer.send(&dispatch("ISSUE-2", None).signed(TUNNEL_SECRET)).await;
    assert_eq!(next(&mut h.peer).await.msg_type, "task.ack");
    let result = next(&mut h.peer).await;
    assert_eq!(result.payload["status"], "failed", "{result:?}");
    assert_eq!(result.payload["summary"], "cli disabled by feature flag or safe mode");
    assert!(
        wait_lines(&h.runs, 1, Duration::from_millis(500)).is_empty(),
        "the executor ran"
    );
}

/// A tunnel task names its agent by id, and only a `cli` agent can run it.
#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_task_runs_only_a_cli_agent() {
    let mut h = start(
        Options {
            extra_agents: "[[agents]]\nid = \"notifier\"\nname = \"Notifier\"\nagent_type = \"custom\"\n\
                           [agents.custom]\ncommand = \"true\"\n",
            ..Options::default()
        },
        true,
    )
    .await;
    h.peer
        .send(&dispatch("ISSUE-3", Some("notifier")).signed(TUNNEL_SECRET))
        .await;
    assert_eq!(next(&mut h.peer).await.msg_type, "task.ack");
    let result = next(&mut h.peer).await;
    assert_eq!(result.payload["status"], "failed", "{result:?}");
    assert_eq!(result.payload["summary"], "no cli agent configured");

    h.peer
        .send(&dispatch("ISSUE-4", Some("runner")).signed(TUNNEL_SECRET))
        .await;
    assert_eq!(next(&mut h.peer).await.msg_type, "task.ack");
    assert_eq!(next(&mut h.peer).await.payload["status"], "success");
}

/// At most `cli_max_concurrency` tunnel tasks run at once.
#[tokio::test(flavor = "multi_thread")]
async fn tunnel_tasks_respect_cli_max_concurrency() {
    let mut h = start(
        Options {
            cli_max_concurrency: 1,
            ..Options::default()
        },
        false,
    )
    .await;
    h.peer.send(&dispatch("ISSUE-5", None).signed(TUNNEL_SECRET)).await;
    h.peer.send(&dispatch("ISSUE-6", None).signed(TUNNEL_SECRET)).await;
    assert_eq!(
        wait_lines(&h.runs, 4, Duration::from_secs(15)),
        ["start", "end", "start", "end"],
        "tunnel tasks overlapped"
    );
}

/// The terminal callback of a tunnel task is sent only with `[features].callback_enabled`.
#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_task_sends_its_callback_only_when_callbacks_are_enabled() {
    for enabled in [false, true] {
        let mut h = start(
            Options {
                callback_enabled: enabled,
                ..Options::default()
            },
            true,
        )
        .await;
        h.peer.send(&dispatch("ISSUE-7", None).signed(TUNNEL_SECRET)).await;
        assert_eq!(next(&mut h.peer).await.msg_type, "task.ack");
        assert_eq!(next(&mut h.peer).await.payload["status"], "success");
        let callback = h.api.next(Duration::from_millis(700)).await;
        if enabled {
            let callback = callback.expect("callback when enabled");
            assert_eq!(callback.body["issue_id"], "ISSUE-7");
            assert_eq!(callback.body["state"], "done");
        } else {
            assert!(callback.is_none(), "callback sent while disabled: {callback:?}");
        }
    }
}

/// SIGTERM while a tunnel task runs: the executor is stopped, the control plane gets a
/// `cancelled` result and the callback reports the failure state.
#[tokio::test(flavor = "multi_thread")]
async fn sigterm_reports_a_running_tunnel_task_as_cancelled() {
    let bin = TempDir::new("bin");
    let started = bin.file("started");
    write_script(
        bin.path(),
        "codex",
        &format!("echo $$ > '{}'\nsleep 30", started.display()),
    );
    let mut api = StubApi::start().await;
    let plane = ControlPlane::bind().await;
    let mut server = Server::start(
        CANONICAL,
        &config(&plane, &api, &Options::default()),
        &[("PATH", &path_with(bin.path()))],
    )
    .expect("server starts");
    let mut peer = plane.accept(WAIT).await;

    peer.send(&dispatch("ISSUE-8", None).signed(TUNNEL_SECRET)).await;
    assert_eq!(next(&mut peer).await.msg_type, "task.ack");
    assert_eq!(wait_lines(&started, 1, WAIT).len(), 1, "executor started");

    server.terminate();
    let result = next(&mut peer).await;
    assert_eq!(result.msg_type, "task.result", "{result:?}");
    assert_eq!(result.payload["status"], "cancelled", "{result:?}");
    let callback = api.next(WAIT).await.expect("terminal callback");
    assert_eq!(callback.body["status"], "cancelled");
    assert_eq!(callback.body["state"], "todo");
    let (code, _) = server.wait_exit(WAIT).expect("server exits");
    assert_eq!(code, Some(0));
    let pids = crate::support::read_pids(&started);
    assert!(crate::support::wait_all_dead(&pids, Duration::from_secs(1)).is_empty());
}

/// A message that is not an envelope is answered with `invalid_json`.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_that_is_not_an_envelope_is_answered_invalid_json() {
    let mut h = start(Options::default(), true).await;
    h.peer.send_text("{not json").await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.msg_type, "error");
    assert_eq!(refused.payload["reason"], "invalid_json");
}

/// A signed envelope is still refused when it is addressed to another agent, is older (or
/// newer) than five minutes, or repeats an envelope id already accepted.
#[tokio::test(flavor = "multi_thread")]
async fn misaddressed_stale_and_replayed_envelopes_are_refused() {
    let mut h = start(Options::default(), true).await;

    let mut other = dispatch("ISSUE-9", None);
    other.agent_id = "someone-else".into();
    h.peer.send(&other.signed(TUNNEL_SECRET)).await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.payload["reason"], "wrong_agent", "{refused:?}");

    let mut stale = dispatch("ISSUE-10", None);
    stale.ts -= 3600;
    h.peer.send(&stale.signed(TUNNEL_SECRET)).await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.payload["reason"], "stale_envelope", "{refused:?}");

    let once = dispatch("ISSUE-11", None).signed(TUNNEL_SECRET);
    h.peer.send(&once).await;
    assert_eq!(next(&mut h.peer).await.msg_type, "task.ack");
    assert_eq!(next(&mut h.peer).await.payload["status"], "success");
    h.peer.send(&once).await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.payload["reason"], "duplicate_envelope", "{refused:?}");
    assert_eq!(
        wait_lines(&h.runs, 3, Duration::from_millis(500)).len(),
        2,
        "only one run"
    );
}
