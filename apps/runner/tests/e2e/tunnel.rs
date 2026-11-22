//! The tunnel through the real executable and an in-process control plane: every inbound
//! envelope goes through the production verification and gating in the tunnel loop.

use crate::support::{ControlPlane, Envelope, RUNNER, SECRET, Server, TunnelPeer};
use serde_json::json;
use std::time::Duration;

const AGENT_ID: &str = "agent-1";
const TUNNEL_SECRET: &str = "tunnel-secret-for-tests";
const WAIT: Duration = Duration::from_secs(15);

#[derive(Default)]
struct Options {
    require_inbound_sig: bool,
}

/// What a `task.dispatch` is answered with: the runner runs no task over the tunnel any more.
const NO_TASKS: &str =
    "the runner no longer runs tasks over the tunnel: agent_type = cli was removed; use agent_type = runner";

fn config(plane: &ControlPlane, options: &Options) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [features]\ntunnel_enabled = true\n\n\
         [tunnel]\nenabled = true\nurl = \"{url}\"\nagent_id = \"{AGENT_ID}\"\nauth_token = \"tunnel-token\"\n\
         reconnect_secs = 1\nheartbeat_secs = 30\nhmac_secret = \"{TUNNEL_SECRET}\"\nrequire_inbound_sig = {require}\n",
        url = plane.url(),
        require = options.require_inbound_sig,
    )
}

struct Harness {
    _server: Server,
    peer: TunnelPeer,
}

async fn start(options: Options) -> Harness {
    let plane = ControlPlane::bind().await;
    let server = Server::start(RUNNER, &config(&plane, &options), &[]).expect("server starts");
    let peer = plane.accept(WAIT).await;
    Harness { _server: server, peer }
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
    let mut h = start(Options::default()).await;
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
    assert_eq!(result.payload["status"], "failed", "{result:?}");
    assert_eq!(result.payload["summary"], NO_TASKS);
    assert_eq!(result.payload["issue_id"], "ISSUE-1");
    assert_signed_by_service(&result);
}

/// With `require_inbound_sig = true` an envelope without `sig` is refused as
/// `missing_signature`; without it, the same envelope is refused as `bad_signature`.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_envelope_is_refused_with_the_configured_reason() {
    for (require, reason) in [(true, "missing_signature"), (false, "bad_signature")] {
        let mut h = start(Options {
            require_inbound_sig: require,
        })
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
    }
}

/// A message that is not an envelope is answered with `invalid_json`.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_that_is_not_an_envelope_is_answered_invalid_json() {
    let mut h = start(Options::default()).await;
    h.peer.send_text("{not json").await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.msg_type, "error");
    assert_eq!(refused.payload["reason"], "invalid_json");
}

/// A signed envelope is still refused when it is addressed to another agent, is older (or
/// newer) than five minutes, or repeats an envelope id already accepted.
#[tokio::test(flavor = "multi_thread")]
async fn misaddressed_stale_and_replayed_envelopes_are_refused() {
    let mut h = start(Options::default()).await;

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
    assert_eq!(next(&mut h.peer).await.payload["status"], "failed");
    h.peer.send(&once).await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.payload["reason"], "duplicate_envelope", "{refused:?}");
    assert!(
        h.peer.recv(Duration::from_millis(500)).await.is_none(),
        "the replay was dispatched again"
    );
}

/// Signature verification comes before the replay memory: an envelope whose signature fails is
/// not remembered, so the same envelope id, correctly signed, still runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_wrongly_signed_envelope_does_not_use_up_its_id() {
    let mut h = start(Options::default()).await;
    let envelope = dispatch("ISSUE-12", None);

    let forged = envelope.clone().signed("not-the-tunnel-secret");
    h.peer.send(&forged).await;
    let refused = next(&mut h.peer).await;
    assert_eq!(refused.payload["reason"], "bad_signature", "{refused:?}");

    let genuine = envelope.signed(TUNNEL_SECRET);
    assert_eq!(genuine.id, forged.id);
    h.peer.send(&genuine).await;
    let ack = next(&mut h.peer).await;
    assert_eq!(ack.msg_type, "task.ack", "{ack:?}");
    assert_eq!(next(&mut h.peer).await.payload["status"], "failed");
}

/// An `error` envelope from the peer is never answered with an `error` of our own, whatever is
/// wrong with it, so two ends that both report refusals cannot keep answering each other.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_error_envelope_is_not_answered() {
    let mut h = start(Options::default()).await;
    let forged = Envelope::new("error", AGENT_ID, json!({"reason": "bad_signature"})).signed("not-the-tunnel-secret");
    h.peer.send(&forged).await;
    let mut misaddressed = Envelope::new("error", "someone-else", json!({"reason": "wrong_agent"}));
    misaddressed = misaddressed.signed(TUNNEL_SECRET);
    h.peer.send(&misaddressed).await;
    assert!(
        h.peer.recv(Duration::from_millis(700)).await.is_none(),
        "an error envelope was answered"
    );

    h.peer.send(&dispatch("ISSUE-13", None).signed(TUNNEL_SECRET)).await;
    assert_eq!(next(&mut h.peer).await.msg_type, "task.ack");
}
