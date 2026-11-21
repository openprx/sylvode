//! Start-up: what the service logs and what it refuses to start with.

use crate::support::{RUNNER, SECRET, Server};

fn minimal_config() -> String {
    format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n")
}

/// The listening line names the address actually bound, so a port chosen by the system
/// (`:0`) is reported as the real port.
#[test]
fn reports_the_bound_address() {
    let server = Server::start(RUNNER, &minimal_config(), &[]).expect("server starts");
    assert_ne!(server.addr.port(), 0, "the real port is logged");
}

/// A successful start logs the listening line with the address that was bound, apart from the
/// timestamp and the port the system chose.
#[test]
fn successful_start_logs_the_bound_address() {
    fn normalized() -> Vec<String> {
        let mut server = Server::start(RUNNER, &minimal_config(), &[]).expect("server starts");
        let port = format!(":{}", server.addr.port());
        let lines = server
            .stdout()
            .iter()
            .map(|line| {
                line.split_once(' ')
                    .map_or(line.as_str(), |(_, rest)| rest)
                    .replace(&port, ":PORT")
            })
            .collect();
        drop(server);
        lines
    }
    let canonical = normalized();
    assert!(
        canonical
            .iter()
            .any(|line| line.contains("listening on 127.0.0.1:PORT")),
        "{canonical:?}"
    );
}

/// The real executable refuses a blank secret instead of accepting requests signed with an
/// empty key.
#[test]
fn refuses_a_blank_webhook_secret() {
    for secrets in [r#"[""]"#, r#"["real-secret", "  "]"#] {
        let config = format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = {secrets}\n");
        let refused = Server::start(RUNNER, &config, &[]).err().expect("start-up is refused");
        assert_eq!(refused.code, Some(1), "{refused:?}");
        assert!(
            refused
                .stdout
                .iter()
                .any(|line| line.contains("security.webhook_secrets[")),
            "{refused:?}"
        );
        assert!(refused.stderr.is_empty(), "{refused:?}");
    }
}

/// `config.example.toml` refuses to start as shipped, because of its placeholder secret, and
/// starts once a real secret is filled in.
#[test]
fn the_shipped_example_refuses_until_its_secret_is_set() {
    let example = include_str!("../../config.example.toml").replace("127.0.0.1:9090", "127.0.0.1:0");
    let refused = Server::start(RUNNER, &example, &[]).err().expect("refused as shipped");
    assert_eq!(refused.code, Some(1));
    assert!(
        refused
            .stdout
            .iter()
            .any(|line| line.contains("security.webhook_secrets[0] is still the example placeholder")),
        "{refused:?}"
    );

    let filled = example.replace("replace-with-the-secret-of-your-sylvode-runner", "a-real-random-secret");
    let server = Server::start(RUNNER, &filled, &[]).expect("starts with a real secret");
    assert!(server.addr.ip().is_loopback());
}

fn unsigned_config(listen: &str, extra: &str) -> String {
    format!("[server]\nlisten = \"{listen}\"\n\n[security]\nallow_unsigned = true\n{extra}\n")
}

/// `allow_unsigned = true` starts on loopback with a prominent warning, is refused on any other
/// address, and starts there, still warning, only when explicitly acknowledged.
#[test]
fn allow_unsigned_warns_and_is_refused_off_loopback_unless_acknowledged() {
    let mut server = Server::start(RUNNER, &unsigned_config("127.0.0.1:0", ""), &[]).expect("loopback starts");
    assert!(
        server
            .stdout()
            .iter()
            .any(|line| line.contains(" WARN ") && line.contains("signatures are NOT verified")),
        "{:?}",
        server.stdout()
    );
    drop(server);

    let refused = Server::start(RUNNER, &unsigned_config("0.0.0.0:0", ""), &[])
        .err()
        .expect("non-loopback unsigned is refused");
    assert_eq!(refused.code, Some(1));
    assert!(
        refused
            .stdout
            .iter()
            .any(|line| line.contains("security.allow_unsigned_non_loopback = true")),
        "{refused:?}"
    );

    let mut acknowledged = Server::start(
        RUNNER,
        &unsigned_config("0.0.0.0:0", "allow_unsigned_non_loopback = true"),
        &[],
    )
    .expect("acknowledged non-loopback starts");
    assert!(
        acknowledged
            .stdout()
            .iter()
            .any(|line| line.contains(" WARN ") && line.contains("anyone who can reach 0.0.0.0:0")),
        "{:?}",
        acknowledged.stdout()
    );
}
