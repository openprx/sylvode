use std::time::Duration;

use super::build_http_client;
use super::config::{RunnerConfig, RunnerHttpConfig};

fn parse(toml: &str) -> Result<RunnerConfig, String> {
    let config: RunnerConfig = toml::from_str(toml).map_err(|e| e.to_string())?;
    config.validate()?;
    Ok(config)
}

#[test]
fn an_empty_runner_block_takes_the_tuned_defaults() {
    let config = parse("").expect("empty block");
    assert_eq!(config.enqueue_answer_ms, 250);
    assert_eq!(config.max_queued_per_session, 100);
    let http = &config.http;
    assert_eq!(
        (
            http.pool_idle_timeout_secs,
            http.pool_max_idle_per_host,
            http.tcp_keepalive_secs,
            http.connect_timeout_secs
        ),
        (60, 8, 30, 5)
    );
}

#[test]
fn an_existing_block_with_a_name_entry_loads_unchanged() {
    let config = parse(
        r#"
state_dir = "~/state"
allowed_roots = ["~/work"]
heartbeat_secs = 300

[tmux]
socket_name = "svw"

[[tmux.sessions]]
bot = "00000000-0000-4000-8000-000000000001"
cli = "claude"
name = "sv"

[mcp]
tool_mode = "search"
"#,
    )
    .expect("a block written for the runner before the pool");
    assert_eq!(config.tmux.sessions.len(), 1);
    assert_eq!(config.tmux.sessions[0].name, "sv");
    assert_eq!(config.enqueue_answer_ms, 250);
    assert_eq!(config.http.pool_idle_timeout_secs, 60);
}

#[test]
fn the_new_keys_parse() {
    let config = parse(
        r#"
enqueue_answer_ms = 300
max_queued_per_session = 50

[http]
pool_idle_timeout_secs = 45
pool_max_idle_per_host = 4
tcp_keepalive_secs = 15
connect_timeout_secs = 3
"#,
    )
    .expect("the enabled values");
    assert_eq!((config.enqueue_answer_ms, config.max_queued_per_session), (300, 50));
    assert_eq!(
        (
            config.http.pool_idle_timeout_secs,
            config.http.pool_max_idle_per_host,
            config.http.tcp_keepalive_secs,
            config.http.connect_timeout_secs
        ),
        (45, 4, 15, 3)
    );
}

#[test]
fn out_of_range_values_and_unknown_keys_are_refused_by_name() {
    for (toml, key) in [
        ("enqueue_answer_ms = 60001", "enqueue_answer_ms"),
        ("max_queued_per_session = 100001", "max_queued_per_session"),
        ("[http]\npool_idle_timeout_secs = 0", "pool_idle_timeout_secs"),
        ("[http]\npool_max_idle_per_host = 1025", "pool_max_idle_per_host"),
        ("[http]\ntcp_keepalive_secs = 3601", "tcp_keepalive_secs"),
        ("[http]\nconnect_timeout_secs = 601", "connect_timeout_secs"),
        ("[http]\nmystery = 1", "mystery"),
    ] {
        let err = parse(toml).expect_err(toml);
        assert!(err.contains(key), "{toml:?} -> {err}");
    }
}

#[test]
fn the_client_builds_with_the_defaults_and_with_every_option_set() {
    assert!(build_http_client(&RunnerHttpConfig::default()).is_ok());
    let tuned = RunnerHttpConfig {
        pool_idle_timeout_secs: 60,
        pool_max_idle_per_host: 8,
        tcp_keepalive_secs: 30,
        connect_timeout_secs: 5,
    };
    assert!(tuned.validate().is_ok());
    assert!(build_http_client(&tuned).is_ok());
    assert_eq!(
        Duration::from_secs(RunnerHttpConfig::default().pool_idle_timeout_secs),
        Duration::from_secs(60)
    );
    // The values that switch an option off are still valid.
    let off = RunnerHttpConfig {
        pool_idle_timeout_secs: 90,
        pool_max_idle_per_host: 0,
        tcp_keepalive_secs: 0,
        connect_timeout_secs: 0,
    };
    assert!(off.validate().is_ok());
    assert!(build_http_client(&off).is_ok());
}
