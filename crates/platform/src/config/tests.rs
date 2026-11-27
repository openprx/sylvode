//! Behavioural tests for the file backed configuration.

use std::path::{Path, PathBuf};

use super::{
    AppConfig, ConfigError, DEFAULT_CONFIG_PATH, LogFormat, LogOutput, McpToolMode, McpTransport, OpenPrConfig,
    REDACTED, StdoutRole, StorageBackend,
};

const WORKSPACE: &str = "0f8a1b2c-3d4e-4f60-8182-93a4b5c6d7e8";

fn origin() -> PathBuf {
    PathBuf::from("config/sylvode.toml")
}

fn parse(source: &str) -> Result<OpenPrConfig, ConfigError> {
    OpenPrConfig::parse(source, &origin())
}

fn issues(source: &str) -> Vec<String> {
    match parse(source) {
        Ok(_) => panic!("configuration should not have validated"),
        Err(ConfigError::Invalid { issues, .. }) => issues,
        Err(other) => panic!("expected a validation failure, got {other}"),
    }
}

/// A file that exercises every section, used as the base for the negative cases.
fn full_config() -> String {
    format!(
        r#"
[server]
app_name = "api"
bind_addr = "0.0.0.0:8081"

[database]
url = "postgres://openpr:s3cret@localhost:5432/openpr"
max_connections = 40
min_connections = 4
connect_timeout_seconds = 7
idle_timeout_seconds = 45
acquire_timeout_seconds = 6

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
access_ttl_seconds = 3600
refresh_ttl_seconds = 604800

[logging]
filter = "api=debug,tower_http=info"
format = "text"
output = "stdout"

[storage]
backend = "s3"
dir = "./uploads"

[storage.s3]
endpoint = "https://s3.example.com"
bucket = "sylvode-uploads"
region = "eu-central-1"
access_key_id = "AKIAEXAMPLE"
secret_access_key = "s3-secret-access-key"
session_token = "s3-session-token"

[audit]
operation_log_retention_days = 45

[ai_tasks]
processing_timeout_secs = 7200

[migrations]
replay = true
continue_on_error = true

[outbound]
allowed_hosts = ["worker.internal", "hooks.example.com:8443"]
allow_private = true

[mcp]
api_url = "http://api:8080"
bot_token = "opr_live_botexampletoken"
workspace_id = "{WORKSPACE}"
transport = "http"
bind_addr = "0.0.0.0:8090"
"#
    )
}

// ---- happy path ----

#[test]
fn a_complete_file_parses_into_every_section() {
    let config = parse(&full_config()).expect("the complete example should validate");

    assert_eq!(config.server.app_name.as_deref(), Some("api"));
    assert_eq!(config.server.bind_addr.as_deref(), Some("0.0.0.0:8081"));

    let database = config.database_runtime().expect("the database section is complete");
    assert_eq!(database.url.expose(), "postgres://openpr:s3cret@localhost:5432/openpr");
    assert_eq!(database.max_connections, 40);
    assert_eq!(database.min_connections, 4);
    assert_eq!(database.connect_timeout_seconds, 7);
    assert_eq!(database.idle_timeout_seconds, 45);
    assert_eq!(database.acquire_timeout_seconds, 6);

    let auth = config.auth_runtime().expect("the auth section is complete");
    assert_eq!(auth.jwt_secret.expose(), "0123456789abcdef0123456789abcdef");
    assert_eq!(auth.access_ttl_seconds, 3600);
    assert_eq!(auth.refresh_ttl_seconds, 604_800);

    assert_eq!(config.logging.filter.as_deref(), Some("api=debug,tower_http=info"));
    assert_eq!(config.logging.format, LogFormat::Text);
    assert_eq!(config.logging.output, LogOutput::Stdout);

    assert_eq!(config.storage.backend, StorageBackend::S3);
    let s3 = config.storage.s3.as_ref().expect("s3 section should be present");
    assert_eq!(s3.endpoint, "https://s3.example.com");
    assert_eq!(s3.bucket, "sylvode-uploads");
    assert_eq!(s3.region, "eu-central-1");
    assert_eq!(s3.secret_access_key.expose(), "s3-secret-access-key");
    assert_eq!(
        s3.session_token.as_ref().map(super::Secret::expose),
        Some("s3-session-token")
    );
    assert_eq!(config.audit.operation_log_retention_days, 45);
    assert_eq!(config.ai_tasks.processing_timeout_secs, 7200);

    assert!(config.migrations.replay);
    assert!(config.migrations.continue_on_error);

    assert_eq!(
        config.outbound.allowed_hosts,
        vec!["worker.internal", "hooks.example.com:8443"]
    );
    assert!(config.outbound.allow_private);
    assert_eq!(
        config.outbound.allowlist_csv(),
        "worker.internal,hooks.example.com:8443"
    );

    assert_eq!(config.mcp.transport, McpTransport::Http);
    assert!(config.mcp.transport.is_networked());
    let mcp = config.mcp_runtime().expect("mcp section is complete");
    assert_eq!(mcp.api_url, "http://api:8080");
    assert_eq!(
        mcp.bot_token.as_ref().map(super::Secret::expose),
        Some("opr_live_botexampletoken")
    );
    assert_eq!(mcp.workspace_id.map(|id| id.to_string()).as_deref(), Some(WORKSPACE));
    assert_eq!(mcp.bind_addr, "0.0.0.0:8090");
}

#[test]
fn a_minimal_file_falls_back_to_documented_defaults() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect("database and auth alone should be enough");

    assert!(config.server.app_name.is_none());
    assert!(config.server.bind_addr.is_none());
    let database = config.database_runtime().expect("the url alone is enough");
    assert_eq!(database.max_connections, 20);
    assert_eq!(database.min_connections, 2);
    assert_eq!(database.connect_timeout_seconds, 5);
    assert_eq!(database.idle_timeout_seconds, 30);
    assert_eq!(database.acquire_timeout_seconds, 5);
    let auth = config.auth_runtime().expect("the secret alone is enough");
    assert_eq!(auth.access_ttl_seconds, 1_296_000);
    assert_eq!(auth.refresh_ttl_seconds, 1_728_000);
    assert_eq!(config.logging.format, LogFormat::Json);
    assert_eq!(config.logging.output, LogOutput::Stderr);
    assert_eq!(
        config.logging.filter_or_default("worker"),
        "worker=info,tower_http=info"
    );
    assert_eq!(config.storage.backend, StorageBackend::Local);
    assert_eq!(config.storage.dir, Path::new("./uploads"));
    assert_eq!(config.audit.operation_log_retention_days, 30);
    assert_eq!(config.ai_tasks.processing_timeout_secs, 3600);
    assert!(!config.migrations.replay);
    assert!(!config.migrations.continue_on_error);
    assert!(config.outbound.allowed_hosts.is_empty());
    assert!(!config.outbound.allow_private);
    assert_eq!(config.mcp.transport, McpTransport::Stdio);
}

#[test]
fn operation_log_retention_days_are_bounded() {
    for days in [0, 3_651] {
        let reported = issues(&format!("[audit]\noperation_log_retention_days = {days}\n"));
        assert!(
            reported
                .iter()
                .any(|issue| issue.contains("audit.operation_log_retention_days")),
            "invalid retention {days} was not rejected: {reported:?}"
        );
    }
}

#[test]
fn ai_task_processing_timeout_is_bounded() {
    for secs in [0, 59, 604_801] {
        let reported = issues(&format!("[ai_tasks]\nprocessing_timeout_secs = {secs}\n"));
        assert!(
            reported
                .iter()
                .any(|issue| issue.contains("ai_tasks.processing_timeout_secs")),
            "invalid timeout {secs} was not rejected: {reported:?}"
        );
    }
    for secs in [60, 604_800] {
        let config = parse(&format!("[ai_tasks]\nprocessing_timeout_secs = {secs}\n"))
            .unwrap_or_else(|error| panic!("valid timeout {secs} was rejected: {error}"));
        assert_eq!(config.ai_tasks.processing_timeout_secs, secs);
    }
}

#[test]
fn ai_task_chain_depth_defaults_and_is_bounded() {
    let config = parse("").expect("an empty file is valid");
    assert_eq!(config.ai_tasks.max_chain_depth, 3);
    for depth in [0, 11] {
        let reported = issues(&format!("[ai_tasks]\nmax_chain_depth = {depth}\n"));
        assert!(
            reported.iter().any(|issue| issue.contains("ai_tasks.max_chain_depth")),
            "invalid chain depth {depth} was not rejected: {reported:?}"
        );
    }
    for depth in [1, 10] {
        let config = parse(&format!("[ai_tasks]\nmax_chain_depth = {depth}\n"))
            .unwrap_or_else(|error| panic!("valid chain depth {depth} was rejected: {error}"));
        assert_eq!(config.ai_tasks.max_chain_depth, depth);
    }
}

#[test]
fn ai_task_lease_settings_default_and_are_bounded() {
    let config = parse("").expect("an empty file is valid");
    assert_eq!(config.ai_tasks.lease_extension_secs, 1_800);
    assert_eq!(config.ai_tasks.max_processing_secs, 86_400);

    for secs in [0, 59, 86_401] {
        let reported = issues(&format!("[ai_tasks]\nlease_extension_secs = {secs}\n"));
        assert!(
            reported
                .iter()
                .any(|issue| issue.contains("ai_tasks.lease_extension_secs")),
            "invalid lease extension {secs} was not rejected: {reported:?}"
        );
    }
    for secs in [60, 86_400] {
        let config = parse(&format!("[ai_tasks]\nlease_extension_secs = {secs}\n"))
            .unwrap_or_else(|error| panic!("valid lease extension {secs} was rejected: {error}"));
        assert_eq!(config.ai_tasks.lease_extension_secs, secs);
    }

    let reported = issues("[ai_tasks]\nmax_processing_secs = 604801\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("ai_tasks.max_processing_secs")),
        "{reported:?}"
    );
    for secs in [3_600, 604_800] {
        let config = parse(&format!("[ai_tasks]\nmax_processing_secs = {secs}\n"))
            .unwrap_or_else(|error| panic!("valid cap {secs} was rejected: {error}"));
        assert_eq!(config.ai_tasks.max_processing_secs, secs);
    }
}

#[test]
fn ai_task_cap_below_the_timeout_names_both_keys() {
    let reported = issues("[ai_tasks]\nprocessing_timeout_secs = 7200\nmax_processing_secs = 3600\n");
    assert!(
        reported.iter().any(|issue| {
            issue.contains("ai_tasks.max_processing_secs") && issue.contains("ai_tasks.processing_timeout_secs")
        }),
        "{reported:?}"
    );
    let config = parse("[ai_tasks]\nprocessing_timeout_secs = 7200\nmax_processing_secs = 7200\n")
        .expect("a cap equal to the timeout is valid");
    assert_eq!(config.ai_tasks.max_processing_secs, 7_200);
}

#[test]
fn an_unknown_ai_tasks_key_is_still_refused() {
    assert!(parse("[ai_tasks]\nlease_extension = 60\n").is_err());
}

// ---- one error report for every problem ----

#[test]
fn every_value_the_api_is_missing_is_reported_in_one_pass() {
    let config = parse("[server]\napp_name = \"api\"\n").expect("presence is not a load time question");
    let Err(ConfigError::Invalid { issues: reported, .. }) = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
    else {
        panic!("the api must refuse to start without a database and a signing key");
    };
    assert!(
        reported.iter().any(|issue| issue.contains("database.url is required")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("auth.jwt_secret is required")),
        "{reported:?}"
    );
    assert_eq!(reported.len(), 2, "exactly the two missing values, got {reported:?}");
}

#[test]
fn unrelated_problems_across_sections_are_all_reported_together() {
    let reported = issues(
        r#"
[server]
bind_addr = "0.0.0.0:not-a-port"

[database]
min_connections = 90
max_connections = 10
connect_timeout_seconds = 0

[auth]
jwt_secret = "short"
access_ttl_seconds = 0

[logging]
format = "yaml"

[storage]
backend = "gcs"

[outbound]
allowed_hosts = ["https://hooks.example.com/path", "*.example.com"]

[mcp]
api_url = "ftp://api"
bot_token = "not-prefixed"
workspace_id = "not-a-uuid"
transport = "grpc"

"#,
    );

    for expected in [
        "server.bind_addr",
        "database.min_connections",
        "database.connect_timeout_seconds",
        "auth.jwt_secret must be at least",
        "auth.access_ttl_seconds",
        "logging.format",
        "storage.backend",
        "outbound.allowed_hosts entry https://hooks.example.com/path",
        "outbound.allowed_hosts entry *.example.com",
        "mcp.api_url",
        "mcp.bot_token",
        "mcp.workspace_id",
        "mcp.transport",
    ] {
        assert!(
            reported.iter().any(|issue| issue.contains(expected)),
            "missing {expected:?} in {reported:?}"
        );
    }
    assert!(reported.len() >= 13, "expected every problem at once, got {reported:?}");
}

#[test]
fn the_rendered_error_lists_all_issues_and_points_at_the_example() {
    let config = parse("[server]\napp_name = \"api\"\n").expect("presence is not a load time question");
    let rendered = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
        .expect_err("should fail")
        .to_string();
    assert!(rendered.contains("2 unusable values"), "{rendered}");
    assert!(rendered.contains("database.url is required"), "{rendered}");
    assert!(rendered.contains("auth.jwt_secret is required"), "{rendered}");
    assert!(rendered.contains("config/sylvode.example.toml"), "{rendered}");
    assert!(rendered.contains(&origin().display().to_string()), "{rendered}");
}

// ---- one file, three binaries: presence is the caller's question ----

#[test]
fn an_mcp_only_file_needs_neither_a_database_nor_a_signing_key() {
    let config = parse(&format!(
        r#"
[mcp]
bot_token = "opr_live_token"
workspace_id = "{WORKSPACE}"
"#
    ))
    .expect("an MCP-only deployment must not have to invent a database URL and a signing key");

    let mcp = config.mcp_runtime().expect("the mcp section is complete");
    assert_eq!(mcp.workspace_id.map(|id| id.to_string()).as_deref(), Some(WORKSPACE));
    assert_eq!(mcp.api_url, super::DEFAULT_MCP_API_URL);
    assert!(config.database.url.is_none());
    assert!(config.auth.jwt_secret.is_none());
}

#[test]
fn a_binary_that_opens_the_pool_still_refuses_a_file_without_a_database() {
    let config = parse(&format!(
        r#"
[mcp]
bot_token = "opr_live_token"
workspace_id = "{WORKSPACE}"
"#
    ))
    .expect("the file itself is valid");

    let Err(ConfigError::Invalid { issues, .. }) = config.database_runtime() else {
        panic!("database_runtime must refuse an absent [database] section");
    };
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues.iter().any(|issue| issue.contains("database.url is required")),
        "{issues:?}"
    );

    let Err(ConfigError::Invalid { issues, .. }) = config.auth_runtime() else {
        panic!("auth_runtime must refuse an absent [auth] section");
    };
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues.iter().any(|issue| issue.contains("auth.jwt_secret is required")),
        "{issues:?}"
    );
}

#[test]
fn a_present_but_unusable_database_url_is_still_refused_at_load_time() {
    // Laziness applies to presence only: a value that *is* written down is checked as before, so
    // a typo never survives until the first connection attempt.
    let reported = issues("[database]\nurl = \"postgres://openpr:${PGPASSWORD}@db/openpr\"\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("database.url must be a concrete database URL")),
        "{reported:?}"
    );

    let reported = issues("[auth]\njwt_secret = \"short\"\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("auth.jwt_secret must be at least")),
        "{reported:?}"
    );
}

#[test]
fn a_database_section_without_a_url_still_validates_its_pool_shape() {
    let reported = issues("[database]\nmin_connections = 90\nmax_connections = 10\n");
    assert!(
        reported.iter().any(|issue| issue.contains("database.min_connections")),
        "{reported:?}"
    );
    assert!(
        !reported.iter().any(|issue| issue.contains("database.url")),
        "an absent url is the caller's question, not the file's: {reported:?}"
    );
}

// ---- logging ----

#[test]
fn logging_writes_to_stderr_unless_the_file_asks_for_stdout() {
    let config = parse("[logging]\nformat = \"text\"\n").expect("logging alone is a valid file");
    assert_eq!(config.logging.output, LogOutput::Stderr);
    assert_eq!(config.logging.effective_output(StdoutRole::Free), LogOutput::Stderr);

    let config = parse("[logging]\noutput = \"stdout\"\n").expect("stdout is a valid choice");
    assert_eq!(config.logging.output, LogOutput::Stdout);
    assert_eq!(config.logging.effective_output(StdoutRole::Free), LogOutput::Stdout);
}

#[test]
fn a_stdio_transport_keeps_its_stdout_even_when_the_file_asks_for_stdout_logs() {
    let config = parse("[logging]\noutput = \"stdout\"\n").expect("stdout is a valid choice");
    assert_eq!(
        config.logging.effective_output(StdoutRole::Protocol),
        LogOutput::Stderr,
        "a log line on a JSON-RPC channel corrupts the frame, so the file cannot ask for it"
    );
}

#[test]
fn an_unknown_logging_output_is_refused_rather_than_guessed() {
    let reported = issues("[logging]\noutput = \"syslog\"\n");
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("logging.output must be stderr or stdout")),
        "{reported:?}"
    );
}

// ---- placeholders ----

#[test]
fn placeholder_values_are_refused() {
    for placeholder in [
        "",
        "   ",
        "replace_with_postgres_password",
        "${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD}",
        "change-me-in-production",
    ] {
        let reported = issues(&format!(
            "[database]\nurl = \"{placeholder}\"\n\n[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\n"
        ));
        assert!(
            reported.iter().any(|issue| issue.contains("database.url")),
            "placeholder {placeholder:?} should have been refused, got {reported:?}"
        );
    }

    for placeholder in [
        "",
        "change-me-in-production",
        "replace_with_long_random_secret",
        "${JWT_SECRET:?set JWT_SECRET for Sylvode services}",
    ] {
        let reported = issues(&format!(
            "[database]\nurl = \"postgres://localhost/openpr\"\n\n[auth]\njwt_secret = \"{placeholder}\"\n"
        ));
        assert!(
            reported.iter().any(|issue| issue.contains("auth.jwt_secret")),
            "placeholder {placeholder:?} should have been refused, got {reported:?}"
        );
    }
}

#[test]
fn the_shipped_example_is_refused_because_it_is_all_placeholders() {
    let example = include_str!("../../../../config/sylvode.example.toml");
    let reported = issues(example);
    assert!(
        reported.iter().any(|issue| issue.contains("database.url")),
        "{reported:?}"
    );
    assert!(
        reported.iter().any(|issue| issue.contains("auth.jwt_secret")),
        "{reported:?}"
    );
}

#[test]
fn the_shipped_example_is_structurally_valid_toml_for_this_schema() {
    // Placeholders must fail *validation*, not parsing: an operator who fills them in must get a
    // working file without having to also fix the shape.
    let example = include_str!("../../../../config/sylvode.example.toml");
    let filled = example
        .replace("replace_with_postgres_password", "s3cret")
        .replace("replace_with_a_64_character_hex_secret", &"a".repeat(64))
        .replace("replace_with_s3_access_key_id", "AKIAEXAMPLE")
        .replace("replace_with_s3_secret_access_key", "s3-secret")
        .replace("replace_with_opr_bot_token", "opr_live_token")
        .replace("replace_with_mcp_inbound_token", "mcp-inbound-token-value")
        .replace("replace_with_connector_credential", "connector-credential");
    let config = parse(&filled).expect("the example should validate once the placeholders are replaced");
    let database = config
        .database_runtime()
        .expect("the filled in example names a database");
    assert_eq!(database.url.expose(), "postgres://openpr:s3cret@localhost:5432/openpr");
    AppConfig::from_config(&config, "api", "0.0.0.0:8081").expect("the filled in example is enough to start the api");
}

// ---- uuids ----

#[test]
fn malformed_and_nil_uuids_are_refused_everywhere_they_appear() {
    for field in ["mcp.workspace_id"] {
        for (value, marker) in [
            ("not-a-uuid", "not a valid UUID"),
            ("00000000-0000-0000-0000-000000000000", "nil UUID"),
            ("replace_with_workspace_uuid", "placeholder"),
        ] {
            let section = format!(
                "[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\n\n[mcp]\nworkspace_id = \"{value}\"\n"
            );
            let reported = issues(&format!(
                "[database]\nurl = \"postgres://localhost/openpr\"\n\n{section}"
            ));
            assert!(
                reported
                    .iter()
                    .any(|issue| issue.contains(field) && issue.contains(marker)),
                "{field} = {value:?} should be refused with {marker:?}, got {reported:?}"
            );
        }
    }
}

// ---- tokens ----

/// The shared inbound secret is gone: a file that still carries `auth_token` is refused as an
/// unknown key (the value is never echoed), like any other key nothing reads.
#[test]
fn a_retired_mcp_auth_token_is_an_unknown_key_and_never_echoed() {
    let error = parse(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[mcp]
workspace_id = "{WORKSPACE}"
bot_token = "opr_live_token"
auth_token = "an-inbound-secret-value"
"#
    ))
    .expect_err("an mcp key that nothing reads is refused");
    let text = error.to_string();
    assert!(text.contains("unknown field") && text.contains("auth_token"), "{text}");
    assert!(
        !text.contains("an-inbound-secret-value"),
        "the retired secret must never be echoed: {text}"
    );
}

/// `mcp.bot_token` is the identity stdio acts as, so stdio cannot run without it — and the
/// networked transports never use it, so they must not be made to configure one. A file
/// carrying nothing but a transport and a workspace is a complete http deployment.
#[test]
fn a_networked_transport_needs_no_bot_token_but_stdio_does() {
    let networked = parse(&format!(
        r#"
[mcp]
workspace_id = "{WORKSPACE}"
transport = "http"
bind_addr = "0.0.0.0:8090"
"#
    ))
    .expect("an http deployment needs neither a database nor a bot token");
    let runtime = networked
        .mcp_runtime()
        .expect("a networked transport should resolve without a bot token");
    assert!(runtime.bot_token.is_none());
    assert_eq!(runtime.bind_addr, "0.0.0.0:8090");

    let stdio = parse(&format!(
        r#"
[mcp]
workspace_id = "{WORKSPACE}"
transport = "stdio"
"#
    ))
    .expect("the shape of the file is fine; the missing value is reported by mcp_runtime");
    let Err(ConfigError::Invalid { issues, .. }) = stdio.mcp_runtime() else {
        panic!("stdio without a bot token has no identity to act as and must be refused");
    };
    assert!(issues.iter().any(|issue| issue.contains("mcp.bot_token")), "{issues:?}");
}

#[test]
fn an_incomplete_mcp_section_reports_everything_it_still_needs() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect("the api and the worker do not need [mcp]");

    let Err(ConfigError::Invalid { issues, .. }) = config.mcp_runtime() else {
        panic!("mcp_runtime should refuse an absent [mcp] section");
    };
    assert!(issues.iter().any(|issue| issue.contains("mcp.bot_token")), "{issues:?}");
    assert!(
        issues.iter().any(|issue| issue.contains("mcp.workspace_id")),
        "{issues:?}"
    );
}

#[test]
fn the_workspace_is_required_by_stdio_only() {
    let stdio = parse("[mcp]\nbot_token = \"opr_live_token\"\n").expect("the shape of the file is fine");
    let Err(ConfigError::Invalid { issues, .. }) = stdio.mcp_runtime() else {
        panic!("stdio has no caller token to take the workspace from and must be refused without one");
    };
    assert!(
        issues.iter().any(|issue| issue.contains("mcp.workspace_id")),
        "{issues:?}"
    );

    for transport in ["http", "sse"] {
        let networked = parse(&format!("[mcp]\ntransport = \"{transport}\"\n")).expect("the shape of the file is fine");
        let runtime = networked
            .mcp_runtime()
            .expect("a networked transport takes the workspace from each caller's token");
        assert!(runtime.workspace_id.is_none(), "{transport}");
    }
}

#[test]
fn an_mcp_section_without_optional_values_gets_the_documented_defaults() {
    let config = parse(&format!(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[mcp]
workspace_id = "{WORKSPACE}"
bot_token = "opr_live_token"
"#
    ))
    .expect("bot token and workspace are enough");
    let mcp = config.mcp_runtime().expect("runtime should resolve");
    assert_eq!(mcp.api_url, super::DEFAULT_MCP_API_URL);
    assert_eq!(mcp.bind_addr, super::DEFAULT_MCP_BIND_ADDR);
    assert_eq!(mcp.transport, McpTransport::Stdio);
}

// ---- storage ----

#[test]
fn only_local_and_s3_name_a_storage_backend() {
    for alias in ["filesystem", "fs", "s3-compatible", "s3_compatible"] {
        let reported = issues(&format!(
            r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "{alias}"
"#
        ));
        assert!(
            reported
                .iter()
                .any(|issue| issue.contains("storage.backend must be local or s3")),
            "{alias}: {reported:?}"
        );
    }
}

#[test]
fn the_s3_backend_requires_its_own_section() {
    let reported = issues(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "s3"
"#,
    );
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("[storage.s3] section is required")),
        "{reported:?}"
    );
}

#[test]
fn the_s3_backend_reports_every_missing_credential_at_once() {
    let reported = issues(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "s3"

[storage.s3]
bucket = "UPPERCASE"
"#,
    );
    for expected in [
        "storage.s3.endpoint is required",
        "storage.s3.bucket UPPERCASE may only contain lowercase",
        "storage.s3.access_key_id is required",
        "storage.s3.secret_access_key is required",
    ] {
        assert!(
            reported.iter().any(|issue| issue.contains(expected)),
            "missing {expected:?} in {reported:?}"
        );
    }
}

#[test]
fn a_leftover_s3_section_under_the_local_backend_is_ignored() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"

[storage]
backend = "local"
dir = "/var/lib/sylvode/uploads"

[storage.s3]
bucket = "still-here"
"#,
    )
    .expect("an unread [storage.s3] section must not block the local backend");
    assert_eq!(config.storage.backend, StorageBackend::Local);
    assert_eq!(config.storage.dir, Path::new("/var/lib/sylvode/uploads"));
    assert!(config.storage.s3.is_none());
}

// ---- structural failures ----

#[test]
fn an_unknown_key_is_refused_rather_than_silently_ignored() {
    let error = parse(
        r#"
[database]
url = "postgres://localhost/openpr"
maxconnections = 40

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect_err("a typo must not be accepted");
    let rendered = error.to_string();
    assert!(matches!(error, ConfigError::Malformed { .. }), "{rendered}");
    assert!(rendered.contains("maxconnections"), "{rendered}");
}

#[test]
fn a_parse_failure_never_reproduces_the_offending_line() {
    let error = parse("[auth]\njwt_secret = \"unterminated-super-secret\n").expect_err("should fail");
    let rendered = error.to_string();
    assert!(!rendered.contains("unterminated-super-secret"), "{rendered}");
    assert!(rendered.contains("may hold a secret"), "{rendered}");
}

#[test]
fn a_missing_file_says_where_it_looked_and_how_to_create_it() {
    let error = OpenPrConfig::load(Some(Path::new("/nonexistent/sylvode-does-not-exist.toml")))
        .expect_err("a missing file must not fall back to defaults");
    assert!(matches!(error, ConfigError::NotFound { .. }));
    let rendered = error.to_string();
    assert!(
        rendered.contains("/nonexistent/sylvode-does-not-exist.toml"),
        "{rendered}"
    );
    assert!(rendered.contains(DEFAULT_CONFIG_PATH), "{rendered}");
    assert!(rendered.contains("openssl rand -hex 32"), "{rendered}");
}

#[test]
fn a_directory_in_place_of_the_file_is_reported_as_unreadable() {
    let error = OpenPrConfig::load(Some(Path::new("/"))).expect_err("a directory is not a configuration file");
    assert!(
        matches!(error, ConfigError::Unreadable { .. } | ConfigError::Malformed { .. }),
        "{error}"
    );
}

// ---- projection onto AppConfig ----

#[test]
fn app_config_takes_the_file_over_the_binary_defaults() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let cfg = AppConfig::from_config(&config, "worker", "127.0.0.1:9999").expect("the file names both values");
    assert_eq!(cfg.app_name, "api");
    assert_eq!(cfg.bind_addr, "0.0.0.0:8081");
    assert_eq!(cfg.jwt_secret.expose(), "0123456789abcdef0123456789abcdef");
    assert_eq!(cfg.jwt_access_ttl_seconds, 3600);
    assert_eq!(cfg.jwt_refresh_ttl_seconds, 604_800);
}

#[test]
fn app_config_falls_back_to_the_binary_defaults_when_the_file_is_silent() {
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
"#,
    )
    .expect("minimal file");
    let cfg = AppConfig::from_config(&config, "worker", "0.0.0.0:8081").expect("the file names both values");
    assert_eq!(cfg.app_name, "worker");
    assert_eq!(cfg.bind_addr, "0.0.0.0:8081");
}

// ---- auth.allow_insecure_cookies (Secure attribute fail-closed gate) ----

#[test]
fn cookies_stay_secure_by_default() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081").expect("the file names both values");
    assert!(!cfg.allow_insecure_cookies);
}

#[test]
fn allow_insecure_cookies_is_accepted_on_an_explicit_loopback_bind() {
    let config = parse(
        r#"
[server]
bind_addr = "127.0.0.1:8081"

[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
allow_insecure_cookies = true
"#,
    )
    .expect("minimal file");
    let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
        .expect("a loopback bind_addr paired with the explicit opt-in must start");
    assert!(cfg.allow_insecure_cookies);
    assert_eq!(cfg.bind_addr, "127.0.0.1:8081");
}

#[test]
fn allow_insecure_cookies_fails_closed_on_a_non_loopback_bind() {
    let config = parse(&full_config().replace(
        "jwt_secret = \"0123456789abcdef0123456789abcdef\"",
        "jwt_secret = \"0123456789abcdef0123456789abcdef\"\nallow_insecure_cookies = true",
    ))
    .expect("the complete example should validate at the parse stage");
    // `full_config()`'s bind_addr is "0.0.0.0:8081", which is not loopback.
    let error = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
        .expect_err("a non-loopback bind_addr must refuse to start with insecure cookies allowed");
    let ConfigError::Invalid { issues, .. } = error else {
        panic!("expected ConfigError::Invalid, got {error}");
    };
    assert!(
        issues.iter().any(|issue| issue.contains("allow_insecure_cookies")),
        "{issues:?}"
    );
}

#[test]
fn allow_insecure_cookies_fails_closed_when_bind_addr_is_left_to_the_binary_default() {
    // No `[server]` section at all: `bind_addr` resolves to the binary's own default, which for
    // every real binary in this workspace is a non-loopback wildcard bind. A missing bind_addr
    // must not be silently treated as "safe enough" just because the operator wrote nothing.
    let config = parse(
        r#"
[database]
url = "postgres://localhost/openpr"

[auth]
jwt_secret = "0123456789abcdef0123456789abcdef"
allow_insecure_cookies = true
"#,
    )
    .expect("minimal file");
    let error = AppConfig::from_config(&config, "api", "0.0.0.0:8081")
        .expect_err("an unset bind_addr must resolve through the (non-loopback) binary default and fail closed");
    assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
}

// ---- redaction ----

#[test]
fn debug_output_of_app_config_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081").expect("the file names both values");
    let rendered = format!("{cfg:?}");
    assert!(!rendered.contains("0123456789abcdef0123456789abcdef"), "{rendered}");
    assert!(!rendered.contains("s3cret"), "{rendered}");
    assert_eq!(rendered.matches(REDACTED).count(), 2, "{rendered}");
    assert!(rendered.contains("app_name"), "{rendered}");
}

#[test]
fn debug_output_of_the_whole_config_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    for rendered in [format!("{config:?}"), format!("{config:#?}")] {
        for plaintext in [
            "0123456789abcdef0123456789abcdef",
            "s3cret",
            "postgres://openpr",
            "s3-secret-access-key",
            "s3-session-token",
            "opr_live_botexampletoken",
            "mcp-inbound-token-value",
            "shipping-credential",
            "payments-credential",
            "other-tenant-credential",
        ] {
            assert!(!rendered.contains(plaintext), "{plaintext} leaked into {rendered}");
        }
        assert!(rendered.contains(REDACTED), "{rendered}");
    }
}

#[test]
fn debug_output_of_the_database_and_auth_runtimes_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let database = config.database_runtime().expect("the database section is complete");
    let auth = config.auth_runtime().expect("the auth section is complete");
    for rendered in [format!("{database:?}"), format!("{auth:?}")] {
        assert!(!rendered.contains("s3cret"), "{rendered}");
        assert!(!rendered.contains("postgres://openpr"), "{rendered}");
        assert!(!rendered.contains("0123456789abcdef0123456789abcdef"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
    }
}

#[test]
fn debug_output_of_the_mcp_runtime_never_contains_a_secret() {
    let config = parse(&full_config()).expect("the complete example should validate");
    let mcp = config.mcp_runtime().expect("mcp section is complete");
    let rendered = format!("{mcp:?}");
    assert!(!rendered.contains("opr_live_botexampletoken"), "{rendered}");
    assert!(!rendered.contains("mcp-inbound-token-value"), "{rendered}");
    assert!(rendered.contains("http://api:8080"), "{rendered}");
}

#[test]
fn only_loopback_addresses_and_localhost_count_as_a_loopback_bind() {
    for bind in [
        "127.0.0.1:8081",
        "127.10.20.30:8081",
        "[::1]:8081",
        "localhost:8081",
        "127.0.0.1",
    ] {
        assert!(super::raw::is_loopback_bind_addr(bind), "{bind} is loopback");
    }
    for bind in [
        "127.example.org:8081",
        "127.0.0.1.example.org:8081",
        "127.attacker:8081",
        "0.0.0.0:8081",
        "[::]:8081",
        "192.0.2.1:8081",
        "localhost.example.org:8081",
    ] {
        assert!(!super::raw::is_loopback_bind_addr(bind), "{bind} is not loopback");
    }
}

// ---- flow.conversion_reap_interval_secs ----

#[test]
fn the_conversion_reap_interval_defaults_to_sixty_seconds_and_is_read_from_flow() {
    let defaulted = parse("").unwrap_or_else(|err| panic!("an empty file validates: {err}"));
    assert_eq!(defaulted.flow.conversion_reap_interval_secs, 60);
    let configured = parse("[flow]\nconversion_reap_interval_secs = 2\n")
        .unwrap_or_else(|err| panic!("a two-second interval validates: {err}"));
    assert_eq!(configured.flow.conversion_reap_interval_secs, 2);
}

#[test]
fn a_conversion_reap_interval_outside_one_second_to_one_day_is_refused() {
    for value in ["0", "86401"] {
        let reported = issues(&format!("[flow]\nconversion_reap_interval_secs = {value}\n"));
        assert!(
            reported
                .iter()
                .any(|issue| issue.contains("flow.conversion_reap_interval_secs must be between 1 and 86400")),
            "{value}: {reported:?}"
        );
    }
}

#[test]
fn the_incremental_window_is_off_by_default_and_can_be_switched_on() {
    assert!(!parse("").unwrap().flow.incremental_window);
    assert!(!parse("[flow]\n").unwrap().flow.incremental_window);
    assert!(
        parse("[flow]\nincremental_window = true\n")
            .unwrap()
            .flow
            .incremental_window
    );
    assert!(
        !parse("[flow]\nincremental_window = false\n")
            .unwrap()
            .flow
            .incremental_window
    );
    assert!(
        parse("[flow]\nincremental_window = \"on\"\n").is_err(),
        "a non-boolean value is refused"
    );
}

#[test]
fn the_setup_cpu_budget_defaults_to_2500_and_is_bounded() {
    assert_eq!(parse("").unwrap().flow.setup_cpu_budget_ms, 2_500);
    for value in [500_u64, 2_500, 3_000, 4_000] {
        let configured = parse(&format!("[flow]\nsetup_cpu_budget_ms = {value}\n")).unwrap();
        assert_eq!(configured.flow.setup_cpu_budget_ms, value);
    }
    for value in ["0", "499", "4001", "100000", "-1", "\"2500\"", "2.5"] {
        assert!(
            parse(&format!("[flow]\nsetup_cpu_budget_ms = {value}\n")).is_err(),
            "{value} is refused"
        );
    }
}

#[test]
fn flow_body_asset_limit_is_bounded_and_configurable() {
    let defaulted = parse("").unwrap();
    assert_eq!(defaulted.flow.body_asset_max_bytes, 52_428_800);
    let configured = parse("[flow]\nbody_asset_max_bytes = 4096\n").unwrap();
    assert_eq!(configured.flow.body_asset_max_bytes, 4096);
    for value in [0, 52_428_801] {
        assert!(
            issues(&format!("[flow]\nbody_asset_max_bytes = {value}\n"))
                .iter()
                .any(|issue| issue.contains("flow.body_asset_max_bytes"))
        );
    }
}

#[test]
fn trusted_proxies_are_validated_and_optional() {
    let absent = parse("[server]\napp_name = \"api\"\n").expect("valid");
    assert!(absent.server.trusted_proxies.is_none());

    let set = parse("[server]\ntrusted_proxies = [\"198.51.100.0/24\", \"::1\"]\n").expect("valid");
    let list = set.server.trusted_proxies.expect("present");
    assert_eq!(list.len(), 2);
    assert!(
        list.first()
            .is_some_and(|block| block.contains("198.51.100.9".parse().unwrap()))
    );

    let found = issues("[server]\ntrusted_proxies = [\"10.0.0.0/40\", \"nope\"]\n");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|issue| issue.contains("server.trusted_proxies")));
}

// ---- [oauth] and the delegated-access keys of [mcp] ----

const FORWARD_SECRET: &str = "forward-secret-for-tests-0123456789abcdef";

fn oauth_file(oauth_body: &str) -> String {
    format!("[oauth]\n{oauth_body}\n")
}

fn oauth_issues(oauth_body: &str) -> Vec<String> {
    issues(&oauth_file(oauth_body))
}

fn assert_refused(body: &str, expected: &str) {
    let reported = oauth_issues(body);
    assert!(
        reported.iter().any(|issue| issue.contains(expected)),
        "expected {expected:?} for {body:?}, got {reported:?}"
    );
}

#[test]
fn the_authorization_server_is_off_unless_an_issuer_is_configured() {
    assert!(parse("").expect("an empty file validates").oauth.is_none());
    assert!(parse("[oauth]\n").expect("an empty section validates").oauth.is_none());
    // Tuning values alone do not turn it on.
    assert!(
        parse(&oauth_file("access_token_ttl_seconds = 300"))
            .expect("validates")
            .oauth
            .is_none()
    );
}

#[test]
fn an_issuer_alone_enables_it_with_every_default() {
    let config = parse(&oauth_file("issuer = \"https://sylvode.example.com\"")).expect("validates");
    let oauth = config.oauth.expect("enabled");
    assert_eq!(oauth.issuer, "https://sylvode.example.com");
    assert_eq!(oauth.api_resource, "https://sylvode.example.com/api");
    assert_eq!(oauth.mcp_resource, None);
    assert!(oauth.internal_forward_secret.is_none());
    assert!(!oauth.allow_insecure_loopback_issuer);
    assert_eq!(oauth.access_token_ttl_seconds, 600);
    assert_eq!(oauth.refresh_token_idle_days, 30);
    assert_eq!(oauth.grant_max_lifetime_days, 365);
    assert_eq!(oauth.authorization_code_ttl_seconds, 60);
    assert_eq!(oauth.max_active_grants_per_user_client, 10);
    assert_eq!(oauth.event_feed_retention_days, 7);
    assert_eq!(oauth.event_backfill_max_events, 1_000);
    assert_eq!(oauth.event_backfill_max_age_hours, 72);
    assert_eq!(oauth.max_streams_per_process, 2_000);
    // The in-memory constructor and the loader agree on every default.
    assert_eq!(
        format!("{oauth:?}"),
        format!("{:?}", super::OAuthConfig::with_defaults("https://sylvode.example.com"))
    );
}

#[test]
fn issuer_rules() {
    let refused = [
        ("issuer = \"http://sylvode.example.com\"", "must use https"),
        (
            "issuer = \"http://localhost:8080\"\nallow_insecure_loopback_issuer = true",
            "must use https",
        ),
        (
            "issuer = \"http://192.0.2.5:8080\"\nallow_insecure_loopback_issuer = true",
            "must use https",
        ),
        ("issuer = \"http://127.0.0.1:18097\"", "allow_insecure_loopback_issuer"),
        (
            "issuer = \"http://127.0.0.1\"\nallow_insecure_loopback_issuer = true",
            "must name a port",
        ),
        ("issuer = \"https://sylvode.example.com/\"", "without a path"),
        ("issuer = \"https://sylvode.example.com/ui\"", "without a path"),
        ("issuer = \"https://sylvode.example.com?x=1\"", "without a path"),
        ("issuer = \"https://sylvode.example.com#frag\"", "fragment"),
        ("issuer = \"https://user:pw@sylvode.example.com\"", "credentials"),
        ("issuer = \"ftp://sylvode.example.com\"", "absolute http or https"),
        ("issuer = \"sylvode.example.com\"", "absolute http or https"),
        ("issuer = \"https://\"", "names no host"),
        ("issuer = \"https://sylvode.example.com:0\"", "invalid port"),
        ("issuer = \"https://syl vode.example.com\"", "whitespace"),
        ("issuer = \"replace_with_issuer\"", "placeholder"),
        ("issuer = \"\"", "placeholder"),
    ];
    for (body, expected) in refused {
        assert_refused(body, expected);
    }
    for body in [
        "issuer = \"https://sylvode.example.com\"",
        "issuer = \"https://sylvode.example.com:8443\"",
        "issuer = \"http://127.0.0.1:18097\"\nallow_insecure_loopback_issuer = true",
        "issuer = \"http://[::1]:18097\"\nallow_insecure_loopback_issuer = true",
        "issuer = \"https://127.0.0.1:8443\"\nallow_insecure_loopback_issuer = true",
    ] {
        parse(&oauth_file(body)).unwrap_or_else(|err| panic!("{body:?} should validate: {err}"));
    }
}

#[test]
fn the_insecure_loopback_exception_is_refused_on_a_non_loopback_issuer() {
    assert_refused(
        "issuer = \"https://sylvode.example.com\"\nallow_insecure_loopback_issuer = true",
        "requires oauth.issuer to name 127.0.0.1 or [::1]",
    );
    assert_refused("allow_insecure_loopback_issuer = true", "oauth.issuer is required");
    // Not setting the flag keeps the issuer https even on loopback.
    parse(&oauth_file("issuer = \"https://127.0.0.1:8443\"")).expect("https on loopback is fine");
}

#[test]
fn resource_and_secret_keys_need_an_issuer_and_each_other() {
    assert_refused(
        "api_resource = \"https://sylvode.example.com/api\"",
        "oauth.issuer is required",
    );
    assert_refused(
        "mcp_resource = \"https://sylvode.example.com/mcp\"",
        "oauth.issuer is required",
    );
    assert_refused(
        &format!("internal_forward_secret = \"{FORWARD_SECRET}\""),
        "oauth.issuer is required",
    );
    let issuer = "issuer = \"https://sylvode.example.com\"";
    assert_refused(
        &format!("{issuer}\nmcp_resource = \"https://sylvode.example.com/mcp\""),
        "oauth.internal_forward_secret is required when oauth.mcp_resource is set",
    );
    assert_refused(
        &format!("{issuer}\ninternal_forward_secret = \"{FORWARD_SECRET}\""),
        "oauth.mcp_resource is required when oauth.internal_forward_secret is set",
    );
    assert_refused(
        &format!("{issuer}\nmcp_resource = \"https://sylvode.example.com/mcp\"\ninternal_forward_secret = \"short\""),
        "at least 32 bytes",
    );
    assert_refused(
        &format!(
            "{issuer}\nmcp_resource = \"https://sylvode.example.com/mcp\"\ninternal_forward_secret = \"replace_with_at_least_32_random_bytes\""
        ),
        "not a placeholder",
    );
    let config = parse(&oauth_file(&format!(
        "{issuer}\nmcp_resource = \"https://sylvode.example.com/mcp\"\ninternal_forward_secret = \"{FORWARD_SECRET}\""
    )))
    .expect("validates")
    .oauth
    .expect("enabled");
    assert_eq!(config.mcp_resource.as_deref(), Some("https://sylvode.example.com/mcp"));
    assert_eq!(
        config.internal_forward_secret.as_ref().map(super::Secret::expose),
        Some(FORWARD_SECRET)
    );
    let rendered = format!("{config:?} {config:#?}");
    assert!(!rendered.contains(FORWARD_SECRET), "{rendered}");
}

#[test]
fn resource_url_rules() {
    let issuer = "issuer = \"https://sylvode.example.com\"";
    assert_refused(
        &format!("{issuer}\napi_resource = \"https://sylvode.example.com/api#x\""),
        "fragment",
    );
    assert_refused(
        &format!("{issuer}\napi_resource = \"http://192.0.2.5/api\""),
        "must use https",
    );
    assert_refused(&format!("{issuer}\napi_resource = \"/api\""), "absolute http or https");
    let loopback = "issuer = \"http://127.0.0.1:18097\"\nallow_insecure_loopback_issuer = true";
    let config = parse(&oauth_file(&format!(
        "{loopback}\napi_resource = \"http://127.0.0.1:18097/api\"\nmcp_resource = \"http://127.0.0.1:8090/mcp/rpc\"\n\
         internal_forward_secret = \"{FORWARD_SECRET}\""
    )))
    .expect("validates")
    .oauth
    .expect("enabled");
    assert_eq!(config.api_resource, "http://127.0.0.1:18097/api");
    assert_eq!(config.mcp_resource.as_deref(), Some("http://127.0.0.1:8090/mcp/rpc"));
}

#[test]
fn every_numeric_setting_is_bounded() {
    let issuer = "issuer = \"https://sylvode.example.com\"";
    let bounds: [(&str, i64, i64); 9] = [
        ("access_token_ttl_seconds", 60, 3_600),
        ("refresh_token_idle_days", 1, 90),
        ("grant_max_lifetime_days", 1, 730),
        ("authorization_code_ttl_seconds", 10, 600),
        ("max_active_grants_per_user_client", 1, 50),
        ("event_feed_retention_days", 1, 30),
        ("event_backfill_max_events", 100, 10_000),
        ("event_backfill_max_age_hours", 1, 168),
        ("max_streams_per_process", 10, 20_000),
    ];
    for (key, min, max) in bounds {
        for ok in [min, max] {
            parse(&oauth_file(&format!("{issuer}\n{key} = {ok}"))).unwrap_or_else(|err| panic!("{key} = {ok}: {err}"));
        }
        for bad in [min - 1, max + 1] {
            assert_refused(
                &format!("{issuer}\n{key} = {bad}"),
                &format!("oauth.{key} must be between {min} and {max}"),
            );
        }
    }
    let config = parse(&oauth_file(&format!(
        "{issuer}\nrefresh_token_idle_days = 7\ngrant_max_lifetime_days = 90"
    )))
    .expect("validates")
    .oauth
    .expect("enabled");
    assert_eq!(
        (config.refresh_token_idle_days, config.grant_max_lifetime_days),
        (7, 90)
    );
}

/// Two settings that were validated and never read are gone: a file that still has either is
/// refused as an unknown key, so nobody believes they do something.
#[test]
fn the_retired_unused_settings_are_unknown_keys() {
    let error = parse(&oauth_file(
        "issuer = \"https://sylvode.example.com\"\nembed_frame_ancestors = [\"https://app.example.com\"]",
    ))
    .expect_err("an oauth key that nothing reads is refused");
    let text = error.to_string();
    assert!(
        text.contains("unknown field") && text.contains("embed_frame_ancestors"),
        "{text}"
    );
    let error = parse(
        "[database]\nurl = \"postgres://localhost/openpr\"\n\n[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\ndefault_author_id = \"a1b2c3d4-e5f6-4890-abcd-ef1234567890\"\n",
    )
    .expect_err("an auth key that nothing reads is refused");
    let text = error.to_string();
    assert!(
        text.contains("unknown field") && text.contains("default_author_id"),
        "{text}"
    );
}

#[test]
fn an_unknown_oauth_key_is_refused() {
    let error = parse(&oauth_file(
        "issuer = \"https://sylvode.example.com\"\nregistration_endpoint = true",
    ))
    .expect_err("unknown keys are refused");
    assert!(error.to_string().contains("registration_endpoint"), "{error}");
}

fn mcp_file(mcp_body: &str) -> String {
    format!("[mcp]\nworkspace_id = \"{WORKSPACE}\"\ntransport = \"http\"\n{mcp_body}\n")
}

#[test]
fn the_mcp_delegated_keys_are_all_or_nothing() {
    let none = parse(&mcp_file("")).expect("validates").mcp_runtime().expect("runtime");
    assert!(none.oauth.is_none());

    let all = parse(&mcp_file(&format!(
        "oauth_issuer = \"https://sylvode.example.com\"\noauth_resource = \"https://sylvode.example.com/mcp\"\n\
         oauth_forward_secret = \"{FORWARD_SECRET}\""
    )))
    .expect("validates")
    .mcp_runtime()
    .expect("runtime");
    let oauth = all.oauth.expect("delegated access is configured");
    assert_eq!(oauth.issuer, "https://sylvode.example.com");
    assert_eq!(oauth.resource, "https://sylvode.example.com/mcp");
    assert_eq!(oauth.forward_secret.expose(), FORWARD_SECRET);
    assert!(!format!("{oauth:?}").contains(FORWARD_SECRET));

    let partial = [
        (
            "oauth_issuer = \"https://sylvode.example.com\"",
            "mcp.oauth_resource, mcp.oauth_forward_secret",
        ),
        (
            "oauth_resource = \"https://sylvode.example.com/mcp\"",
            "mcp.oauth_issuer, mcp.oauth_forward_secret",
        ),
        (
            &format!("oauth_forward_secret = \"{FORWARD_SECRET}\""),
            "mcp.oauth_issuer, mcp.oauth_resource",
        ),
        (
            "oauth_issuer = \"https://sylvode.example.com\"\noauth_resource = \"https://sylvode.example.com/mcp\"",
            "missing: mcp.oauth_forward_secret",
        ),
    ];
    for (body, missing) in partial {
        let config = parse(&mcp_file(body)).expect("shape is fine, presence is the runtime's question");
        let error = config
            .mcp_runtime()
            .expect_err("a partial set refuses to start")
            .to_string();
        assert!(error.contains("set together or not at all"), "{error}");
        assert!(error.contains(missing), "{missing} not in {error}");
    }
}

#[test]
fn the_tool_mode_defaults_to_search_and_writes_are_allowed() {
    let runtime = parse(&mcp_file("")).expect("validates").mcp_runtime().expect("runtime");
    assert_eq!(runtime.tool_mode, McpToolMode::Search);
    assert!(runtime.allow_write_call);
}

#[test]
fn the_tool_mode_and_the_write_switch_are_read_from_the_file() {
    let full = parse(&mcp_file("tool_mode = \"full\"\nallow_write_call = false"))
        .expect("validates")
        .mcp_runtime()
        .expect("runtime");
    assert_eq!(full.tool_mode, McpToolMode::Full);
    assert!(!full.allow_write_call);
    let search = parse(&mcp_file("tool_mode = \" Search \""))
        .expect("validates")
        .mcp_runtime()
        .expect("runtime");
    assert_eq!(search.tool_mode, McpToolMode::Search);
}

#[test]
fn the_listen_switch_origins_and_stream_cap_have_defaults_and_are_read_from_the_file() {
    let defaults = parse(&mcp_file("")).expect("validates").mcp_runtime().expect("runtime");
    assert!(defaults.listen);
    assert!(defaults.allowed_origins.is_empty());
    assert_eq!(defaults.max_listen_streams, 10_000);
    let set = parse(&mcp_file(
        "listen = false\nallowed_origins = [\"https://app.example.com\", \"http://localhost:3000\"]\nmax_listen_streams = 50",
    ))
    .expect("validates")
    .mcp_runtime()
    .expect("runtime");
    assert!(!set.listen);
    assert_eq!(
        set.allowed_origins,
        vec![
            "https://app.example.com".to_string(),
            "http://localhost:3000".to_string()
        ]
    );
    assert_eq!(set.max_listen_streams, 50);
}

#[test]
fn a_malformed_origin_or_a_zero_stream_cap_is_refused() {
    for origin in [
        "app.example.com",
        "https://app.example.com/path",
        "ftp://x",
        "https://",
        "https://u@h",
    ] {
        let reported = issues(&mcp_file(&format!("allowed_origins = [\"{origin}\"]")));
        assert!(
            reported.iter().any(|issue| issue.contains("mcp.allowed_origins")),
            "{origin}: {reported:?}"
        );
    }
    let reported = issues(&mcp_file("max_listen_streams = 0"));
    assert!(
        reported.iter().any(|issue| issue.contains("mcp.max_listen_streams")),
        "{reported:?}"
    );
    assert!(parse(&mcp_file("listen = \"yes\"")).is_err());
}

#[test]
fn an_unknown_tool_mode_or_a_mistyped_switch_is_refused() {
    let reported = issues(&mcp_file("tool_mode = \"everything\""));
    assert!(
        reported
            .iter()
            .any(|issue| issue.contains("mcp.tool_mode must be search or full")),
        "{reported:?}"
    );
    assert!(parse(&mcp_file("allow_write_call = \"yes\"")).is_err());
}

#[test]
fn the_mcp_delegated_keys_are_checked_for_shape() {
    let reported = issues(&mcp_file(
        "oauth_issuer = \"http://sylvode.example.com\"\noauth_resource = \"https://sylvode.example.com/mcp#x\"\n\
         oauth_forward_secret = \"short\"",
    ));
    for expected in ["mcp.oauth_issuer", "mcp.oauth_resource", "mcp.oauth_forward_secret"] {
        assert!(
            reported.iter().any(|issue| issue.contains(expected)),
            "{expected} in {reported:?}"
        );
    }
    // Plain http is for a loopback literal only.
    parse(&mcp_file(&format!(
        "oauth_issuer = \"http://127.0.0.1:18097\"\noauth_resource = \"http://127.0.0.1:8090/mcp/rpc\"\n\
         oauth_forward_secret = \"{FORWARD_SECRET}\""
    )))
    .expect("loopback http validates");
}

#[test]
fn mcp_and_oauth_sections_must_agree_when_both_are_present() {
    let base = |issuer: &str, resource: &str| {
        format!(
            "[oauth]\nissuer = \"https://sylvode.example.com\"\nmcp_resource = \"https://sylvode.example.com/mcp\"\n\
             internal_forward_secret = \"{FORWARD_SECRET}\"\n{}",
            mcp_file(&format!(
                "oauth_issuer = \"{issuer}\"\noauth_resource = \"{resource}\"\noauth_forward_secret = \"{FORWARD_SECRET}\""
            ))
        )
    };
    parse(&base("https://sylvode.example.com", "https://sylvode.example.com/mcp")).expect("they agree");
    let reported = issues(&base("https://other.example.com", "https://other.example.com/mcp"));
    assert!(
        reported
            .iter()
            .any(|i| i.contains("mcp.oauth_issuer must equal oauth.issuer")),
        "{reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|i| i.contains("mcp.oauth_resource must equal oauth.mcp_resource")),
        "{reported:?}"
    );
}

// ---- [inbound] ----

#[test]
fn the_inbound_endpoint_is_off_by_default_with_the_documented_limits() {
    let config = parse("").expect("an empty file validates");
    assert_eq!(config.inbound, super::InboundConfig::default());
    assert!(!config.inbound.enabled);
    assert_eq!(config.inbound.max_body_bytes, 65_536);
    assert_eq!(config.inbound.signature_tolerance_secs, 300);
    assert_eq!(config.inbound.auth_failures_per_minute_per_ip, 30);
    assert_eq!(config.inbound.max_active_alert_tasks_per_workspace, 5);
    assert_eq!(config.inbound.max_alerts_per_rule_per_hour, 60);
    assert_eq!(config.inbound.event_retention_days, 30);
    assert_eq!(config.inbound.followup_interval_secs, 2);
}

#[test]
fn the_inbound_section_reads_every_key() {
    let config = parse(
        "[inbound]\nenabled = true\nmax_body_bytes = 8192\nsignature_tolerance_secs = 60\n\
         auth_failures_per_minute_per_ip = 5\nmax_active_alert_tasks_per_workspace = 3\n\
         max_alerts_per_rule_per_hour = 10\nevent_retention_days = 7\nfollowup_interval_secs = 5\n",
    )
    .expect("valid");
    assert!(config.inbound.enabled);
    assert_eq!(config.inbound.max_body_bytes, 8_192);
    assert_eq!(config.inbound.signature_tolerance_secs, 60);
    assert_eq!(config.inbound.auth_failures_per_minute_per_ip, 5);
    assert_eq!(config.inbound.max_active_alert_tasks_per_workspace, 3);
    assert_eq!(config.inbound.max_alerts_per_rule_per_hour, 10);
    assert_eq!(config.inbound.event_retention_days, 7);
    assert_eq!(config.inbound.followup_interval_secs, 5);
}

#[test]
fn every_inbound_limit_has_an_accepted_range() {
    for (key, bad) in [
        ("max_body_bytes", [4_095_u32, 262_145]),
        ("signature_tolerance_secs", [29, 901]),
        ("auth_failures_per_minute_per_ip", [0, 1_001]),
        ("max_active_alert_tasks_per_workspace", [0, 101]),
        ("max_alerts_per_rule_per_hour", [0, 10_001]),
        ("event_retention_days", [0, 366]),
        ("followup_interval_secs", [0, 61]),
    ] {
        for value in bad {
            let reported = issues(&format!("[inbound]\n{key} = {value}\n"));
            assert!(
                reported.iter().any(|issue| issue.contains(&format!("inbound.{key}"))),
                "{key} = {value} should have been refused, got {reported:?}"
            );
        }
    }
    assert!(
        parse("[inbound]\nbogus = 1\n").is_err(),
        "an unknown inbound key is refused"
    );
}

// ---- [email] and [server] public_url ----

const EMAIL_ON: &str = "[server]\npublic_url = \"https://sylvode.example.com\"\n\n[email]\nenabled = true\n\
     smtp_host = \"smtp.example.com\"\nfrom = \"Sylvode <sylvode@example.com>\"\n";

#[test]
fn email_is_off_by_default() {
    let config = parse("").expect("an empty file validates");
    assert!(config.email.is_none());
    assert!(config.server.public_url.is_none());
    let disabled = parse("[email]\nenabled = false\n").expect("a disabled section needs nothing");
    assert!(disabled.email.is_none());
}

#[test]
fn a_valid_email_section_resolves_its_defaults() {
    let config = parse(EMAIL_ON).expect("valid");
    let email = config.email.expect("enabled");
    assert_eq!(email.smtp_host, "smtp.example.com");
    assert_eq!(email.smtp_port, 587);
    assert_eq!(email.tls, super::EmailTls::Starttls);
    assert_eq!(email.from, "Sylvode <sylvode@example.com>");
    assert_eq!(email.digest_hour_utc, 8);
    assert_eq!(email.public_url, "https://sylvode.example.com");
    assert!(email.username.is_none() && email.password.is_none() && email.ca_file.is_none());
}

#[test]
fn every_email_key_is_read() {
    let source = "[server]\npublic_url = \"HTTPS://Sylvode.Example.com/\"\n\n[email]\nenabled = true\n\
         smtp_host = \"Mail.Example.com\"\nsmtp_port = 2465\ntls = \"implicit\"\nusername = \"sylvode\"\n\
         password = \"hunter2-hunter2\"\nfrom = \"sylvode@example.com\"\nca_file = \"/etc/sylvode/ca.pem\"\n\
         digest_hour_utc = 23\n";
    let email = parse(source).expect("valid").email.expect("enabled");
    assert_eq!(email.public_url, "https://sylvode.example.com");
    assert_eq!(email.smtp_host, "mail.example.com");
    assert_eq!(email.smtp_port, 2465);
    assert_eq!(email.tls, super::EmailTls::Implicit);
    assert_eq!(email.username.as_deref(), Some("sylvode"));
    assert_eq!(
        email.password.as_ref().map(super::Secret::expose),
        Some("hunter2-hunter2")
    );
    assert_eq!(email.ca_file, Some(PathBuf::from("/etc/sylvode/ca.pem")));
    assert_eq!(email.digest_hour_utc, 23);
    assert!(
        !format!("{email:?}").contains("hunter2"),
        "the password is redacted in Debug"
    );
}

#[test]
fn email_enabled_without_public_url_is_refused() {
    let source = "[email]\nenabled = true\nsmtp_host = \"smtp.example.com\"\nfrom = \"sylvode@example.com\"\n";
    let reported = issues(source);
    assert!(reported.iter().any(|i| i.contains("server.public_url")), "{reported:?}");
}

#[test]
fn email_enabled_needs_a_host_and_a_sender() {
    let reported = issues("[server]\npublic_url = \"https://a.example\"\n[email]\nenabled = true\n");
    assert!(reported.iter().any(|i| i.contains("email.smtp_host")), "{reported:?}");
}

#[test]
fn tls_none_is_accepted_only_for_a_loopback_host() {
    let remote = EMAIL_ON.to_string() + "tls = \"none\"\n";
    let reported = issues(&remote);
    assert!(reported.iter().any(|i| i.contains("email.tls")), "{reported:?}");
    for host in ["127.0.0.1", "localhost", "[::1]"] {
        let local = EMAIL_ON.replace("smtp.example.com", host) + "tls = \"none\"\n";
        let email = parse(&local).expect("loopback may skip TLS").email.expect("enabled");
        assert_eq!(email.tls, super::EmailTls::None);
        assert_eq!(email.smtp_port, 25);
    }
}

#[test]
fn bad_email_values_are_refused_by_key() {
    for (key, value) in [
        ("tls", "\"plain\""),
        ("smtp_port", "0"),
        ("smtp_port", "70000"),
        ("digest_hour_utc", "24"),
        ("from", "\"not a mailbox\""),
        ("smtp_host", "\"smtp.example.com:587\""),
        ("username", "\"\""),
        ("ca_file", "\"\""),
    ] {
        let source = format!("{EMAIL_ON}{key} = {value}\n");
        // A duplicate key is a TOML error, so build the section with the key replaced instead.
        let source = if source.matches(&format!("\n{key} = ")).count() > 1 {
            EMAIL_ON
                .lines()
                .filter(|line| !line.starts_with(&format!("{key} =")))
                .collect::<Vec<_>>()
                .join("\n")
                + &format!("\n{key} = {value}\n")
        } else {
            source
        };
        let reported = issues(&source);
        assert!(
            reported.iter().any(|i| i.contains(&format!("email.{key}"))),
            "{key} = {value} should be refused by name, got {reported:?}"
        );
    }
}

#[test]
fn public_url_must_be_an_origin() {
    for bad in [
        "sylvode.example.com",
        "ftp://sylvode.example.com",
        "https://sylvode.example.com/app",
        "https://user:pw@sylvode.example.com",
        "https://sylvode.example.com?x=1",
        "https://",
        "https://sylvode.example.com:0",
    ] {
        let reported = issues(&format!("[server]\npublic_url = \"{bad}\"\n"));
        assert!(
            reported.iter().any(|i| i.contains("server.public_url")),
            "{bad}: {reported:?}"
        );
        assert!(
            reported.iter().all(|i| !i.contains("user:pw")),
            "the value must not appear in the message: {reported:?}"
        );
    }
}

#[test]
fn the_password_and_the_mailbox_never_appear_in_an_error() {
    let source = "[server]\npublic_url = \"https://a.example\"\n[email]\nenabled = true\n\
         smtp_host = \"smtp.example.com\"\nfrom = \"hunter2-hunter2\"\nusername = \"u\"\npassword = \"hunter2-hunter2\"\n\
         tls = \"bogus-hunter2\"\n";
    let error = parse(source).expect_err("invalid");
    let text = format!("{error} {error:?}");
    assert!(!text.contains("hunter2"), "{text}");
    let malformed = parse("[email]\npassword = \"hunter2-hunter2\"\nenabled = \"yes\"\n").expect_err("wrong type");
    let text = format!("{malformed} {malformed:?}");
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn a_password_needs_a_username_and_unknown_email_keys_are_refused() {
    let reported = issues(&(EMAIL_ON.to_string() + "password = \"abc\"\n"));
    assert!(reported.iter().any(|i| i.contains("email.password")), "{reported:?}");
    assert!(parse("[email]\nbogus = 1\n").is_err());
}
