//! Start-up: what the service logs and what it refuses to start with.

use crate::support::{CANONICAL, LEGACY, SECRET, Server};

fn minimal_config() -> String {
    format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n")
}

/// The listening line names the address actually bound, so a port chosen by the system
/// (`:0`) is reported as the real port.
#[test]
fn reports_the_bound_address() {
    let server = Server::start(CANONICAL, &minimal_config(), &[]).expect("server starts");
    assert_ne!(server.addr.port(), 0, "the real port is logged");
}

/// A successful start writes the same stdout under both executable names, apart from the
/// timestamp and the port the system chose.
#[test]
fn successful_start_writes_the_same_stdout_under_both_names() {
    fn normalized(binary: &str) -> Vec<String> {
        let mut server = Server::start(binary, &minimal_config(), &[]).expect("server starts");
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
    let canonical = normalized(CANONICAL);
    assert!(
        canonical
            .iter()
            .any(|line| line.contains("listening on 127.0.0.1:PORT")),
        "{canonical:?}"
    );
    assert_eq!(normalized(LEGACY), canonical);
}

/// The real executable refuses a blank secret instead of accepting requests signed with an
/// empty key.
#[test]
fn refuses_a_blank_webhook_secret() {
    for secrets in [r#"[""]"#, r#"["real-secret", "  "]"#] {
        let config = format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = {secrets}\n");
        let refused = Server::start(CANONICAL, &config, &[])
            .err()
            .expect("start-up is refused");
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
