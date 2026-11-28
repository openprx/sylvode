//! The `/webhook` endpoint through the real executable.

use crate::support::{RUNNER, SECRET, Server, bot_task, post_signed, rfc3339};

fn config() -> String {
    format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n")
}

/// A captured signed delivery cannot be sent again: the second copy is refused with 409.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_delivery_is_refused() {
    let server = Server::start(RUNNER, &config(), &[]).expect("server starts");
    let body = bot_task("Nobody", "replay me").to_string();
    let (first, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(first, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent");
    let (second, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(second, 409, "{answer}");
}

/// Signature verification comes before the replay memory: a request whose signature is missing
/// or wrong is refused without being remembered, so the same delivery, correctly signed, is still
/// accepted once.
#[tokio::test(flavor = "multi_thread")]
async fn an_unverified_request_does_not_use_up_the_delivery_id() {
    use crate::support::{post_signed_by, post_with};

    let server = Server::start(RUNNER, &config(), &[]).expect("server starts");
    let body = bot_task("Nobody", "verify first").to_string();
    let (status, _) = post_with(&server.url("/webhook"), &body, &[]).await;
    assert_eq!(status, 401, "unsigned");
    let (status, _) = post_signed_by(&server.url("/webhook"), &body, "not-the-secret").await;
    assert_eq!(status, 401, "wrongly signed");

    let (status, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(status, 200, "the genuine delivery was refused: {answer}");
    let (status, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(status, 409, "{answer}");
}

/// A signed delivery whose timestamp is older than `max_delivery_age_secs` is refused with 401.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_delivery_is_refused() {
    let server = Server::start(RUNNER, &config(), &[]).expect("server starts");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs();
    let mut body = bot_task("Nobody", "stale");
    body["timestamp"] = serde_json::Value::String(rfc3339(now - 3600));
    let (status, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(status, 401, "{answer}");

    body["timestamp"] = serde_json::Value::String(rfc3339(now - 10));
    let (status, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(status, 200, "{answer}");
}

/// Text from a delivery cannot forge log lines: a newline inside a logged field is escaped.
#[tokio::test(flavor = "multi_thread")]
async fn delivery_text_cannot_forge_a_log_line() {
    let mut server = Server::start(RUNNER, &config(), &[]).expect("server starts");
    let forged = "2026-10-05T00:00:00.000000Z ERROR sylvode_runner::app: FORGED LINE";
    let mut body = bot_task("Nobody\nFORGED BOT", "t");
    body["event"] = serde_json::Value::String(format!("x\n{forged}"));
    let (status, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert!(server.wait_for_line("No agent for bot task", std::time::Duration::from_secs(5)));
    let lines = server.stdout().to_vec();
    assert!(
        lines.iter().any(|line| line.contains("FORGED LINE")),
        "the event is logged: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with(forged) || line.starts_with("FORGED BOT")),
        "a payload newline started a log line of its own: {lines:?}"
    );
}

fn command_agents_config() -> String {
    format!(
        "{}\n[[agents]]\nid = \"printer\"\nname = \"Printer\"\nagent_type = \"custom\"\nmessage_template = \"{{key}}\"\n\
         [agents.custom]\ncommand = \"sh\"\nargs = [\"-c\", \"echo stdout-detail; echo stderr-detail >&2; exit $0\", \"0\"]\n\n\
         [[agents]]\nid = \"failer\"\nname = \"Failer\"\nagent_type = \"custom\"\nmessage_template = \"{{key}}\"\n\
         [agents.custom]\ncommand = \"sh\"\nargs = [\"-c\", \"echo stdout-detail; echo stderr-detail >&2; exit $0\", \"3\"]\n\n\
         [[agents]]\nid = \"notifier\"\nname = \"Notifier\"\nagent_type = \"openclaw\"\nmessage_template = \"{{key}}\"\n\
         [agents.openclaw]\ncommand = \"sh\"\nargs = [\"-c\", \"echo stderr-detail >&2; exit 1\", \"sh\"]\nchannel = \"c\"\ntarget = \"t\"\n",
        config()
    )
}

/// The `/webhook` answer reports how the dispatch went, never the output of the command or the
/// body of a remote error: the sender stores the answer in its delivery log.
#[tokio::test(flavor = "multi_thread")]
async fn the_answer_does_not_echo_command_output() {
    let server = Server::start(RUNNER, &command_agents_config(), &[]).expect("server starts");
    for (bot, expected) in [
        ("Printer", "ok"),
        ("Failer", "error: the command exited with status 3"),
        ("Notifier", "error: the command exited with status 1"),
    ] {
        let (status, answer) = post_signed(&server.url("/webhook"), &bot_task(bot, "t").to_string()).await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["status"], "dispatched", "{answer}");
        assert_eq!(answer["result"], expected, "{bot}: {answer}");
        assert!(!answer.to_string().contains("-detail"), "{bot}: {answer}");
    }
}

/// `command = "openclaw"` with `args = ["message", "send"]` runs `openclaw` with `message send`
/// before the channel options.
#[tokio::test(flavor = "multi_thread")]
async fn the_openclaw_command_runs_with_its_args_before_the_channel_options() {
    use crate::support::{TempDir, path_with, wait_lines, write_script};

    let bin = TempDir::new("bin");
    let argv = bin.file("argv");
    write_script(
        bin.path(),
        "openclaw",
        &format!("for a in \"$@\"; do echo \"$a\" >> '{}'; done", argv.display()),
    );
    let config = format!(
        "{}\n[[agents]]\nid = \"david\"\nname = \"David\"\nagent_type = \"openclaw\"\nmessage_template = \"{{key}}\"\n\
         [agents.openclaw]\ncommand = \"openclaw\"\nargs = [\"message\", \"send\"]\nchannel = \"signal\"\ntarget = \"uuid:u1\"\n",
        config()
    );
    let server = Server::start(RUNNER, &config, &[("PATH", &path_with(bin.path()))]).expect("the configuration starts");
    let (status, answer) = post_signed(&server.url("/webhook"), &bot_task("David", "t").to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["result"], "ok", "{answer}");
    assert_eq!(
        wait_lines(&argv, 8, std::time::Duration::from_secs(5)),
        [
            "message",
            "send",
            "--channel",
            "signal",
            "--target",
            "uuid:u1",
            "--message",
            "BIL-1"
        ]
    );
}

/// A request that never finishes cannot hold up shutdown: with `http_timeout_secs = 1` the bound
/// is 5 + 2 × 1 + 5 = 12 seconds, and a client that sent the headers and 3 of 100 body bytes and
/// then stalled is cut off by then; the service exits 0.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_request_does_not_hold_up_shutdown_beyond_the_bound() {
    use tokio::io::AsyncWriteExt;

    let config = config().replace("[security]", "[runtime]\nhttp_timeout_secs = 1\n\n[security]");
    let mut server = Server::start(RUNNER, &config, &[]).expect("server starts");
    let mut stalled = tokio::net::TcpStream::connect(server.addr).await.expect("connect");
    stalled
        .write_all(
            b"POST /webhook HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"a",
        )
        .await
        .expect("send a partial request");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let begun = std::time::Instant::now();
    server.terminate();
    let (code, _) = server
        .wait_exit(std::time::Duration::from_secs(20))
        .expect("the service exits although a request is still open");
    let took = begun.elapsed();
    assert_eq!(code, Some(0));
    assert!(took < std::time::Duration::from_secs(14), "exited after {took:?}");
    assert!(
        server
            .stdout()
            .iter()
            .any(|line| line.contains("request(s) still open") && line.contains("12s")),
        "{:?}",
        server.stdout()
    );
    drop(stalled);
}

/// A correctly signed request whose timestamp is outside the window is refused and does not use
/// up the delivery id, so the same body signed now is still accepted once.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_signature_timestamp_is_refused() {
    use crate::support::{SECRET, now_secs, post_signed_at};

    let server = Server::start(RUNNER, &config(), &[]).expect("server starts");
    let body = bot_task("Nobody", "stale").to_string();
    let url = server.url("/webhook");
    for skew in [330_u64, 3_600] {
        let (status, _) = post_signed_at(&url, &body, SECRET, now_secs() - skew).await;
        assert_eq!(status, 401, "{skew}s old");
        let (status, _) = post_signed_at(&url, &body, SECRET, now_secs() + skew).await;
        assert_eq!(status, 401, "{skew}s ahead");
    }
    let (status, answer) = post_signed(&url, &body).await;
    assert_eq!(status, 200, "{answer}");
}
