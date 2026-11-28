//! File backed configuration for every Sylvode binary.
//!
//! Sylvode reads no environment variables. One TOML file, divided into sections, configures the
//! API, the worker and the MCP server; each binary consumes the sections it needs and ignores the
//! rest. The file is located by an explicit path only — a `--config <path>` flag, or
//! [`DEFAULT_CONFIG_PATH`] relative to the process working directory — so a deployment is never
//! configured by something that happened to be exported into the process environment.
//!
//! # Loading
//!
//! ```no_run
//! use platform::config::{AppConfig, OpenPrConfig};
//!
//! # fn main() -> Result<(), platform::error::AppError> {
//! // `explicit` is whatever `--config` carried, or `None`.
//! let explicit: Option<std::path::PathBuf> = None;
//! let config = OpenPrConfig::load(explicit.as_deref())?;
//! let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081")?;
//! # let _ = cfg;
//! # Ok(())
//! # }
//! ```
//!
//! # Failure behaviour
//!
//! A missing file is an error, never a silent fallback to defaults: a service that starts with an
//! invented configuration is a service that starts with the wrong database and the wrong signing
//! key. Validation collects *every* unusable value before returning, so one round of edits fixes
//! the whole file.
//!
//! # Eager shape, lazy presence
//!
//! Loading validates the *shape* of every value the file does contain, and refuses the file if
//! any of them is unusable. Whether a *mandatory* value is present is decided by the binary that
//! needs it, through [`OpenPrConfig::database_runtime`], [`OpenPrConfig::auth_runtime`] and
//! [`OpenPrConfig::mcp_runtime`]: one file serves three binaries, and the MCP server reaches the
//! API over HTTP without ever opening a database connection or signing a token. Requiring
//! `database.url` and `auth.jwt_secret` at load time forced an MCP-only deployment to invent two
//! credentials it never uses, which is worse than useless — an invented database URL is a real
//! connection string pointing somewhere unintended.
//!
//! A binary that does need a section still fails to start without it: the accessors report every
//! missing value of the section at once, and [`AppConfig::from_config`] — the API's and the
//! worker's single entry point — reports the missing database *and* auth values together.

mod cidr;
mod email;
mod error;
mod inbound;
mod oauth;
mod packages;
mod raw;
mod secret;
mod site;

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

pub use cidr::IpCidr;
pub use email::{DEFAULT_EMAIL_DIGEST_HOUR_UTC, EmailConfig, EmailTls, MAX_EMAIL_DIGEST_HOUR_UTC};
pub use error::{ConfigError, EXAMPLE_CONFIG_PATH};
pub use inbound::InboundConfig;
pub use oauth::{
    DEFAULT_ACCESS_TOKEN_TTL_SECONDS, DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS, DEFAULT_EVENT_BACKFILL_MAX_AGE_HOURS,
    DEFAULT_EVENT_BACKFILL_MAX_EVENTS, DEFAULT_EVENT_FEED_RETENTION_DAYS, DEFAULT_GRANT_MAX_LIFETIME_DAYS,
    DEFAULT_MAX_ACTIVE_GRANTS_PER_USER_CLIENT, DEFAULT_MAX_STREAMS_PER_PROCESS, DEFAULT_REFRESH_TOKEN_IDLE_DAYS,
    MIN_FORWARD_SECRET_LEN, OAuthConfig,
};
pub use packages::{IndexSource, PackagesConfig};
pub use secret::{REDACTED, Secret};
pub use site::SiteConfig;

/// Where a binary looks for its configuration when no `--config` path is given.
///
/// Relative to the process working directory, so a container that mounts the file at
/// `/app/config/sylvode.toml` and runs with `/app` as its working directory needs no flag.
pub const DEFAULT_CONFIG_PATH: &str = "config/sylvode.toml";

/// Shortest accepted `auth.jwt_secret`.
///
/// Every access and refresh token in the deployment is signed with it; a short value is brute
/// forceable offline from a single captured token.
pub const MIN_JWT_SECRET_LEN: usize = 16;

/// Region assumed when `[storage.s3]` does not name one.
pub const DEFAULT_S3_REGION: &str = "us-east-1";

/// Directory assumed by the local storage backend.
pub const DEFAULT_STORAGE_DIR: &str = "./uploads";

/// Days bot operation records are retained unless `[audit]` overrides it.
pub const DEFAULT_OPERATION_LOG_RETENTION_DAYS: u32 = 30;

/// Seconds an AI task may stay `processing` before the worker ends that attempt.
///
/// Applies unless `[ai_tasks]` overrides it. One hour, the same as the runner's limit for one
/// headless run (`RUN_TIMEOUT_SECS` in `apps/runner/src/runner/headless.rs`).
pub const DEFAULT_AI_TASK_PROCESSING_TIMEOUT_SECS: u64 = 3_600;

/// Accepted range of `ai_tasks.processing_timeout_secs`: one minute to seven days.
pub const AI_TASK_PROCESSING_TIMEOUT_RANGE_SECS: std::ops::RangeInclusive<u64> = 60..=604_800;

/// Seconds each acknowledge, progress report or runner heartbeat extends an active task's lease.
///
/// Applies unless `[ai_tasks]` overrides it.
pub const DEFAULT_AI_TASK_LEASE_EXTENSION_SECS: u64 = 1_800;

/// Accepted range of `ai_tasks.lease_extension_secs`: one minute to one day.
pub const AI_TASK_LEASE_EXTENSION_RANGE_SECS: std::ops::RangeInclusive<u64> = 60..=86_400;

/// Hard cap, in seconds from `started_at`, beyond which no heartbeat extends a task's lease.
///
/// Applies unless `[ai_tasks]` overrides it.
pub const DEFAULT_AI_TASK_MAX_PROCESSING_SECS: u64 = 86_400;

/// Largest accepted `ai_tasks.max_processing_secs`: seven days. The smallest accepted value is the
/// configured `processing_timeout_secs`.
pub const AI_TASK_MAX_PROCESSING_CEILING_SECS: u64 = 604_800;

/// Deepest hop of a bot-to-bot chain.
///
/// A task a human or an inbound event starts is depth 0, a task a bot's action causes while it
/// runs a task is one deeper than that task. A hop beyond this depth is refused. Applies unless
/// `[ai_tasks]` overrides it.
pub const DEFAULT_AI_TASK_MAX_CHAIN_DEPTH: u32 = 3;

/// Accepted range of `ai_tasks.max_chain_depth`.
pub const AI_TASK_MAX_CHAIN_DEPTH_RANGE: std::ops::RangeInclusive<u32> = 1..=10;

/// Bots whose deliveries the worker runs at once (each bot's own tasks go one at a time).
pub const DEFAULT_AI_TASK_DELIVERY_CONCURRENCY: u32 = 8;

/// Accepted range of `ai_tasks.delivery_concurrency`.
pub const AI_TASK_DELIVERY_CONCURRENCY_RANGE: std::ops::RangeInclusive<u32> = 1..=64;

/// Requests in flight to one runner address at once. Only the delivery pool applies it.
pub const DEFAULT_AI_TASK_PER_RUNNER_CONCURRENCY: u32 = 4;

/// Accepted range of `ai_tasks.per_runner_concurrency`.
pub const AI_TASK_PER_RUNNER_CONCURRENCY_RANGE: std::ops::RangeInclusive<u32> = 1..=64;

/// The most tasks one bot's lane picks up in one tick, so the lease clock of a task waiting in a
/// lane stays short.
pub const DEFAULT_AI_TASK_LANE_BATCH: u32 = 10;

/// Milliseconds between the polls of the worker's delivery loop.
pub const DEFAULT_AI_TASK_POLL_INTERVAL_MS: u64 = 1_000;

/// Accepted range of `ai_tasks.poll_interval_ms`.
pub const AI_TASK_POLL_INTERVAL_RANGE_MS: std::ops::RangeInclusive<u64> = 100..=60_000;

/// Accepted range of `ai_tasks.lane_batch`.
pub const AI_TASK_LANE_BATCH_RANGE: std::ops::RangeInclusive<u32> = 1..=1_000;

/// Seconds an idle pooled delivery connection is kept.
pub const DEFAULT_AI_TASK_POOL_IDLE_TIMEOUT_SECS: u64 = 30;

/// Accepted range of `ai_tasks.pool_idle_timeout_secs`.
pub const AI_TASK_POOL_IDLE_TIMEOUT_RANGE_SECS: std::ops::RangeInclusive<u64> = 1..=3_600;

/// Idle pooled delivery connections kept per runner address.
pub const DEFAULT_AI_TASK_POOL_MAX_IDLE_PER_HOST: u32 = 4;

/// Accepted range of `ai_tasks.pool_max_idle_per_host`.
pub const AI_TASK_POOL_MAX_IDLE_PER_HOST_RANGE: std::ops::RangeInclusive<u32> = 1..=64;

/// Seconds between TCP keepalive probes on pooled delivery connections. 0 turns them off.
pub const DEFAULT_AI_TASK_TCP_KEEPALIVE_SECS: u64 = 30;

/// Largest accepted `ai_tasks.tcp_keepalive_secs`.
pub const AI_TASK_TCP_KEEPALIVE_CEILING_SECS: u64 = 3_600;

/// `event_dispatch` expansion-failure retry ceiling assumed unless `[flow]` overrides it.
///
/// `limits-v1.md`'s own `dispatch_max_attempts` rationale ("与 `delivery_max_attempts` 同量级即可")
/// is followed literally here: it is set equal to the one sibling limit the same document *does*
/// freeze (`delivery_max_attempts = 10`), not invented independently.
pub const DEFAULT_FLOW_DISPATCH_MAX_ATTEMPTS: i32 = 10;

/// Seconds between two sweeps of conversion jobs left `started` unless `[flow]` overrides it
/// (`rest-api-v1.md` v0.7: the sweep runs at API startup and then every 60 seconds).
pub const DEFAULT_FLOW_CONVERSION_REAP_INTERVAL_SECS: u64 = 60;

/// The existing upload ceiling, also used by independently owned Page body assets.
pub const DEFAULT_FLOW_BODY_ASSET_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_FLOW_BODY_ASSET_BYTES: u64 = DEFAULT_FLOW_BODY_ASSET_MAX_BYTES;

/// CPU milliseconds the isolated worker's unmetered setup may consume before it yields no base facts.
///
/// Assumed unless `[flow] setup_cpu_budget_ms` overrides it. The setup's own wall timeout is 5 s, so
/// the budget leaves room for the load and for one text over it.
pub const DEFAULT_FLOW_SETUP_CPU_BUDGET_MS: u64 = 2_500;

/// Lower bound accepted for `flow.setup_cpu_budget_ms`: below it even honest pages would lose the
/// incremental window.
pub const MIN_FLOW_SETUP_CPU_BUDGET_MS: u64 = 500;

/// Upper bound accepted for `flow.setup_cpu_budget_ms`: the setup timeout is 5 s of wall time, and
/// a budget at or above 4 s would leave no room for the load and the one text that can run past it.
pub const MAX_FLOW_SETUP_CPU_BUDGET_MS: u64 = 4_000;

/// Upper bound accepted for `flow.conversion_reap_interval_secs` (one day).
pub const MAX_FLOW_CONVERSION_REAP_INTERVAL_SECS: u64 = 86_400;

/// API base URL assumed by the MCP server when `[mcp]` does not name one.
pub const DEFAULT_MCP_API_URL: &str = "http://localhost:8081";

/// Bind address assumed by the MCP server's HTTP and SSE transports.
pub const DEFAULT_MCP_BIND_ADDR: &str = "127.0.0.1:8090";

/// The whole configuration file, parsed and validated.
///
/// `Debug` is derived and is safe: every field carrying credential material is a [`Secret`],
/// which renders as [`REDACTED`].
#[derive(Clone, Debug)]
pub struct OpenPrConfig {
    /// File this configuration was read from, used in diagnostics.
    pub origin: PathBuf,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub audit: AuditConfig,
    pub api: ApiConfig,
    pub ai_tasks: AiTasksConfig,
    pub outbound: OutboundConfig,
    pub mcp: McpConfig,
    pub flow: FlowConfig,
    /// `None` unless `[oauth] issuer` is set: the authorization server is off by default.
    pub oauth: Option<OAuthConfig>,
    /// `[inbound]`: the alert chain's inbound endpoint, off unless `enabled = true`.
    pub inbound: InboundConfig,
    /// `[packages]`: the built-in publisher key and the built-in package directory.
    pub packages: PackagesConfig,
    /// `[site]`: the public site surface switches.
    pub site: SiteConfig,
    /// `[email]`: `Some` only when `enabled = true`, with every value it needs validated.
    pub email: Option<EmailConfig>,
}

impl OpenPrConfig {
    /// Reads and validates the configuration.
    ///
    /// `explicit` is the path a `--config` flag carried. When it is `None`,
    /// [`DEFAULT_CONFIG_PATH`] is used, relative to the process working directory.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let path = explicit.map_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH), Path::to_path_buf);
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::NotFound { path: absolute(&path) });
            }
            Err(err) => {
                return Err(ConfigError::Unreadable {
                    path: absolute(&path),
                    reason: err.to_string(),
                });
            }
        };
        Self::parse(&source, &absolute(&path))
    }

    /// Validates configuration already held in memory. `origin` only labels diagnostics.
    pub fn parse(source: &str, origin: &Path) -> Result<Self, ConfigError> {
        let raw: raw::RawConfig = toml::from_str(source).map_err(|err| ConfigError::from_toml(origin, source, &err))?;
        raw.validate(origin)
    }

    /// The database connection settings, or every reason they are not usable.
    ///
    /// `[database]` is optional for the MCP server, which reaches the API over HTTP and opens no
    /// connection of its own, so presence is checked here rather than at load time; a value that
    /// *is* present is already known to be well formed.
    pub fn database_runtime(&self) -> Result<DatabaseRuntime, ConfigError> {
        self.database.runtime(&self.origin)
    }

    /// The token signing settings, or every reason they are not usable.
    ///
    /// `[auth]` is optional for the MCP server, which presents a bot token issued elsewhere and
    /// signs nothing itself; see [`Self::database_runtime`] for why presence is checked here.
    pub fn auth_runtime(&self) -> Result<AuthRuntime, ConfigError> {
        self.auth.runtime(&self.origin)
    }

    /// The MCP server's runtime settings, or every reason they are not usable.
    ///
    /// `[mcp]` is optional for the API and the worker, so presence is checked here rather than at
    /// load time; a value that *is* present is already known to be well formed.
    pub fn mcp_runtime(&self) -> Result<McpRuntime, ConfigError> {
        self.mcp.runtime(&self.origin)
    }
}

/// Builds the validation error a runtime accessor returns when values are missing.
fn missing_values(origin: &Path, issues: Vec<String>) -> ConfigError {
    ConfigError::Invalid {
        path: origin.to_path_buf(),
        issues,
    }
}

/// Resolves a path against the working directory so diagnostics name a real location.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

/// `[server]` — identity and listen address of an HTTP binary.
///
/// Both values are optional because one file serves three binaries that listen on different
/// ports; each binary supplies its own fallback through [`AppConfig::from_config`].
#[derive(Clone, Debug, Default)]
pub struct ServerConfig {
    pub app_name: Option<String>,
    pub bind_addr: Option<String>,
    /// Reverse proxies whose `X-Forwarded-For` header may be believed (`X-Real-IP` is never read).
    /// `None` means the key was absent and the API applies its built-in default.
    pub trusted_proxies: Option<Vec<IpCidr>>,
    /// Absolute `http(s)` origin that links in e-mails start with, without a trailing slash.
    pub public_url: Option<String>,
}

/// What a binary that opens a database connection is told when `database.url` is absent.
const DATABASE_URL_REQUIRED: &str =
    "database.url is required to open a database connection; add it to the [database] section";

/// What a binary that issues or verifies tokens is told when `auth.jwt_secret` is absent.
const JWT_SECRET_REQUIRED: &str = "auth.jwt_secret is required to sign and verify tokens; add it to the [auth] section (generate one with \
     `openssl rand -hex 32`)";

/// `[database]` — connection string and pool shape.
///
/// `url` is an `Option` because the MCP server shares this file and never connects to the
/// database; [`DatabaseRuntime`] is the shape a binary that does connect asks for.
#[derive(Clone, Debug, Default)]
pub struct DatabaseConfig {
    /// Connection URL. A [`Secret`] because it carries the password.
    pub url: Option<Secret>,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub acquire_timeout_seconds: u64,
}

impl DatabaseConfig {
    /// Why this section cannot open a connection, or `None` when it can.
    const fn missing(&self) -> Option<&'static str> {
        if self.url.is_none() {
            Some(DATABASE_URL_REQUIRED)
        } else {
            None
        }
    }

    fn runtime(&self, origin: &Path) -> Result<DatabaseRuntime, ConfigError> {
        let Some(url) = self.url.as_ref() else {
            return Err(missing_values(origin, vec![DATABASE_URL_REQUIRED.to_owned()]));
        };
        Ok(DatabaseRuntime {
            url: url.clone(),
            max_connections: self.max_connections,
            min_connections: self.min_connections,
            connect_timeout_seconds: self.connect_timeout_seconds,
            idle_timeout_seconds: self.idle_timeout_seconds,
            acquire_timeout_seconds: self.acquire_timeout_seconds,
        })
    }
}

/// `[database]` with the mandatory fields resolved, as a binary that opens the pool needs them.
#[derive(Clone, Debug)]
pub struct DatabaseRuntime {
    pub url: Secret,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub acquire_timeout_seconds: u64,
}

/// `[auth]` — token signing and issuance.
///
/// `jwt_secret` is an `Option` for the same reason as [`DatabaseConfig::url`]: the MCP server
/// presents a bot token issued by the API and signs nothing itself.
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    pub jwt_secret: Option<Secret>,
    pub access_ttl_seconds: i64,
    pub refresh_ttl_seconds: i64,
    /// Explicit developer opt-out of the `Secure` attribute on the access/refresh/clear auth
    /// cookies. Defaults to `false` (cookies are always `Secure`). `AppConfig::from_config`
    /// refuses to start when this is `true` but `server.bind_addr` does not resolve to a loopback
    /// host: skipping `Secure` is only safe when the listener itself is unreachable from outside
    /// the machine, and a missing/non-loopback bind must fail closed rather than silently ship an
    /// insecure cookie to the network.
    pub allow_insecure_cookies: bool,
    /// Multiplier applied to every limit of the console authentication routes (sign-in, register,
    /// refresh, setup status). Validated to be at least 1 by the file loader; a value built by
    /// `Default` is 0 and means 1 to the consumer.
    pub rate_limit_scale: u32,
}

impl AuthConfig {
    /// Why this section cannot sign a token, or `None` when it can.
    const fn missing(&self) -> Option<&'static str> {
        if self.jwt_secret.is_none() {
            Some(JWT_SECRET_REQUIRED)
        } else {
            None
        }
    }

    fn runtime(&self, origin: &Path) -> Result<AuthRuntime, ConfigError> {
        let Some(jwt_secret) = self.jwt_secret.as_ref() else {
            return Err(missing_values(origin, vec![JWT_SECRET_REQUIRED.to_owned()]));
        };
        Ok(AuthRuntime {
            jwt_secret: jwt_secret.clone(),
            access_ttl_seconds: self.access_ttl_seconds,
            refresh_ttl_seconds: self.refresh_ttl_seconds,
        })
    }
}

/// `[auth]` with the mandatory fields resolved, as a binary that issues tokens needs them.
#[derive(Clone, Debug)]
pub struct AuthRuntime {
    pub jwt_secret: Secret,
    pub access_ttl_seconds: i64,
    pub refresh_ttl_seconds: i64,
}

/// Rendering of the tracing subscriber's output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per event, for log shippers.
    #[default]
    Json,
    /// Human readable lines, for a terminal.
    Text,
}

/// Stream the tracing subscriber writes to.
///
/// Defaults to stderr. Stderr is never a data channel, so a log line written there can never
/// corrupt whatever a binary writes to stdout, while every container runtime and init system
/// this project deploys under captures the two streams identically — stderr costs nothing and
/// removes a whole class of failure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogOutput {
    #[default]
    Stderr,
    Stdout,
}

impl LogOutput {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stderr => "stderr",
            Self::Stdout => "stdout",
        }
    }
}

/// What the calling binary's stdout carries, which decides whether it may also carry logs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StdoutRole {
    /// Nothing else is written to stdout, so `logging.output` decides freely.
    #[default]
    Free,
    /// Stdout is a protocol channel — the MCP server's stdio JSON-RPC framing. A single log line
    /// on it corrupts a frame and breaks the session, so logs go to stderr whatever the file says.
    Protocol,
}

/// `[logging]` — replaces `RUST_LOG`.
#[derive(Clone, Debug, Default)]
pub struct LoggingConfig {
    /// `tracing_subscriber` filter directives. `None` means "this binary's own default".
    pub filter: Option<String>,
    pub format: LogFormat,
    /// Requested stream. Honoured only where stdout is not already spoken for; see
    /// [`Self::effective_output`].
    pub output: LogOutput,
}

impl LoggingConfig {
    /// The filter to install, falling back to a per-binary default.
    pub fn filter_or_default(&self, service_name: &str) -> String {
        self.filter
            .clone()
            .unwrap_or_else(|| format!("{service_name}=info,tower_http=info"))
    }

    /// The stream to actually write to, given what the caller's stdout carries.
    ///
    /// The configured value is a preference, not a promise: a process whose stdout carries a
    /// protocol has no stdout to log to, and an operator who writes `output = "stdout"` into a
    /// file shared by three binaries must not be able to break the MCP server's framing with it.
    pub const fn effective_output(&self, stdout: StdoutRole) -> LogOutput {
        match stdout {
            StdoutRole::Free => self.output,
            StdoutRole::Protocol => LogOutput::Stderr,
        }
    }
}

/// Which object storage implementation `[storage]` selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorageBackend {
    /// Files under [`StorageConfig::dir`].
    #[default]
    Local,
    /// An S3 compatible service described by [`StorageConfig::s3`].
    S3,
}

/// `[storage]` — where uploaded objects live.
#[derive(Clone, Debug)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    /// Root directory of the local backend. Ignored when `backend` is `s3`.
    pub dir: PathBuf,
    /// Present and validated whenever `backend` is `s3`.
    pub s3: Option<S3Config>,
}

/// `[audit]` — retention policy for metadata-only operation records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditConfig {
    pub operation_log_retention_days: u32,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            operation_log_retention_days: DEFAULT_OPERATION_LOG_RETENTION_DAYS,
        }
    }
}

/// Applies unless `[api] max_change_hint_streams` overrides it.
pub const DEFAULT_MAX_CHANGE_HINT_STREAMS: u32 = 4_096;

/// Applies unless `[api] max_change_hint_streams_per_workspace` overrides it: a quarter of the
/// process default, so at least four workspaces are needed to fill every place of a process.
pub const DEFAULT_MAX_CHANGE_HINT_STREAMS_PER_WORKSPACE: u32 = 1_024;

/// Accepted range of `api.max_change_hint_streams` and `api.max_change_hint_streams_per_workspace`.
pub const MAX_CHANGE_HINT_STREAMS_CEILING: u32 = 100_000;

/// `[api]` — limits of the API process itself.
///
/// `max_change_hint_streams` caps the open `GET /api/v1/bot/change-hints` streams of one process
/// (each bot is also limited to 64 of its own). `max_change_hint_streams_per_workspace` caps the
/// streams all bots of one workspace may hold together, so a workspace with many bots cannot take
/// every place of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiConfig {
    pub max_change_hint_streams: u32,
    pub max_change_hint_streams_per_workspace: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            max_change_hint_streams: DEFAULT_MAX_CHANGE_HINT_STREAMS,
            max_change_hint_streams_per_workspace: DEFAULT_MAX_CHANGE_HINT_STREAMS_PER_WORKSPACE,
        }
    }
}

/// `[ai_tasks]` — the worker's handling of AI tasks it has delivered.
///
/// `processing_timeout_secs` bounds how long a delivered task may stay `processing` without the
/// receiver completing or failing it. After that the worker ends the attempt: the task goes back
/// to `pending` while attempts remain and is failed otherwise.
///
/// `lease_extension_secs` is how far an acknowledge, a progress report or a runner heartbeat moves
/// the lease of an active task; `max_processing_secs` is the hard cap from `started_at` that no
/// extension passes.
///
/// `max_chain_depth` is the deepest hop of a bot-to-bot chain (see
/// [`DEFAULT_AI_TASK_MAX_CHAIN_DEPTH`]).
///
/// The remaining keys shape the worker's delivery to runners, which always goes through one
/// long-lived client pair whose every new connection is address-checked:
///
/// - `pool_idle_timeout_secs`, `pool_max_idle_per_host` and `tcp_keepalive_secs` shape its
///   connections, and `per_runner_concurrency` caps the requests in flight to one runner address.
/// - The worker delivers on a loop of its own that polls every `poll_interval_ms`. Each bot has at
///   most one lane in flight, which delivers that bot's tasks one at a time in pick-up order, and
///   `delivery_concurrency` is how many bots' lanes run at once. A poll picks up at most
///   `lane_batch` tasks per bot and skips the bots that have a lane in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiTasksConfig {
    pub processing_timeout_secs: u64,
    pub lease_extension_secs: u64,
    pub max_processing_secs: u64,
    pub max_chain_depth: u32,
    pub delivery_concurrency: u32,
    pub per_runner_concurrency: u32,
    pub lane_batch: u32,
    pub poll_interval_ms: u64,
    pub pool_idle_timeout_secs: u64,
    pub pool_max_idle_per_host: u32,
    pub tcp_keepalive_secs: u64,
}

impl Default for AiTasksConfig {
    fn default() -> Self {
        Self {
            processing_timeout_secs: DEFAULT_AI_TASK_PROCESSING_TIMEOUT_SECS,
            lease_extension_secs: DEFAULT_AI_TASK_LEASE_EXTENSION_SECS,
            max_processing_secs: DEFAULT_AI_TASK_MAX_PROCESSING_SECS,
            max_chain_depth: DEFAULT_AI_TASK_MAX_CHAIN_DEPTH,
            delivery_concurrency: DEFAULT_AI_TASK_DELIVERY_CONCURRENCY,
            per_runner_concurrency: DEFAULT_AI_TASK_PER_RUNNER_CONCURRENCY,
            lane_batch: DEFAULT_AI_TASK_LANE_BATCH,
            poll_interval_ms: DEFAULT_AI_TASK_POLL_INTERVAL_MS,
            pool_idle_timeout_secs: DEFAULT_AI_TASK_POOL_IDLE_TIMEOUT_SECS,
            pool_max_idle_per_host: DEFAULT_AI_TASK_POOL_MAX_IDLE_PER_HOST,
            tcp_keepalive_secs: DEFAULT_AI_TASK_TCP_KEEPALIVE_SECS,
        }
    }
}

/// `[flow]` — Sylvode Flow settings `limits-v1.md` marks `status: unset` (the app, not the
/// contract, picks and owns the value; see that file's "冻结前不得填入自造数值" instruction).
///
/// v0.4 only reads `dispatch_max_attempts`: the `event_dispatch.max_attempts` column has no
/// database default (`limits-v1.md`'s `dispatch_max_attempts` row) precisely so this value has to
/// come from a deployment's own configuration rather than an invented literal in the insert
/// statement.
///
/// `collab_allowed_origins` is the `ADR-0007` "严格 Origin allowlist" a collab ticket's `Origin`
/// must belong to, both at ticket issuance and at WebSocket upgrade. Each entry is a normalized
/// `scheme://host[:port]` literal (no path, no wildcard); empty means no origin is allowed
/// (fail closed, matching every other Flow gate's "absent means off" convention).
///
/// `conversion_reap_interval_secs` is the period of the API's sweep that fails conversion jobs
/// left `started` past the stale threshold (`rest-api-v1.md` v0.7).
///
/// `incremental_window` (experimental, default off) lets the collab write window decide the
/// structural limits from the facts of the page and the units an update touched; off, every write
/// runs the full computation, which is what the worker did before the incremental window existed.
///
/// `setup_cpu_budget_ms` is the CPU time the worker's unmetered setup (load, prewarm, base facts)
/// may consume before it yields no facts and the page takes the full window. A slower host than the
/// reference runner may raise it; a setup that times out at 5 s of wall time with at least 80 % of
/// it consumed is treated as an exhausted setup (the full window is tried once), below that as a
/// starved one (retried).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowConfig {
    pub dispatch_max_attempts: i32,
    pub collab_allowed_origins: Vec<String>,
    pub conversion_reap_interval_secs: u64,
    pub body_asset_max_bytes: u64,
    /// Whether the collab write window may decide the structural limits incrementally
    /// (experimental, off by default). Off makes every write run the full computation (the bounded
    /// cost the worker had before the incremental window), whatever the page: the worker's decision is then the full computation's for every
    /// input, except that an update whose tree ops name a parent that does not exist is refused
    /// with either setting (`collab_core::tree_refs`).
    pub incremental_window: bool,
    /// CPU budget of the worker's setup, in milliseconds; validated to the range from
    /// [`MIN_FLOW_SETUP_CPU_BUDGET_MS`] to [`MAX_FLOW_SETUP_CPU_BUDGET_MS`], both included.
    pub setup_cpu_budget_ms: u64,
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            dispatch_max_attempts: DEFAULT_FLOW_DISPATCH_MAX_ATTEMPTS,
            collab_allowed_origins: Vec::new(),
            conversion_reap_interval_secs: DEFAULT_FLOW_CONVERSION_REAP_INTERVAL_SECS,
            body_asset_max_bytes: DEFAULT_FLOW_BODY_ASSET_MAX_BYTES,
            incremental_window: false,
            setup_cpu_budget_ms: DEFAULT_FLOW_SETUP_CPU_BUDGET_MS,
        }
    }
}

/// `[storage.s3]` — credentials and addressing for the S3 compatible backend.
#[derive(Clone, Debug)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: Secret,
    pub secret_access_key: Secret,
    pub session_token: Option<Secret>,
}

/// `[outbound]` — what the delivery pipeline is allowed to call.
#[derive(Clone, Debug, Default)]
pub struct OutboundConfig {
    /// Hosts (`host` or `host:port`) exempt from the private address checks.
    pub allowed_hosts: Vec<String>,
    /// Disables the private address checks entirely. Only for closed networks.
    pub allow_private: bool,
}

impl OutboundConfig {
    /// The allowlist in the comma separated form the host matcher consumes.
    pub fn allowlist_csv(&self) -> String {
        self.allowed_hosts.join(",")
    }
}

/// Transport the MCP server serves on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McpTransport {
    /// A pipe pair owned by the parent process.
    #[default]
    Stdio,
    Http,
    Sse,
}

impl McpTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }

    /// Whether the transport opens a socket that unrelated callers can reach.
    pub const fn is_networked(self) -> bool {
        matches!(self, Self::Http | Self::Sse)
    }
}

/// Which tools `tools/list` offers: the search entry tools (the default) or the whole registry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McpToolMode {
    /// `sylvode_overview`, `sylvode_search`, `sylvode_read`, `sylvode_write` and the task tools.
    #[default]
    Search,
    /// Every registry tool, callable directly by name.
    Full,
}

impl McpToolMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Full => "full",
        }
    }

    /// The mode a value names (`search` or `full`, case-insensitive, surrounding blanks ignored).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "search" => Some(Self::Search),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// `[mcp]` — MCP server settings.
///
/// Every field is optional so that the API and the worker can share the file without declaring a
/// section they do not use. A field that *is* present has already been checked for shape;
/// [`OpenPrConfig::mcp_runtime`] checks that the mandatory ones are there.
#[derive(Clone, Debug, Default)]
pub struct McpConfig {
    pub api_url: Option<String>,
    /// The workspace bot token `stdio` and the CLI subcommands present to the API.
    ///
    /// A `stdio` process is launched by one person's MCP client and speaks for one bot account,
    /// so the configuration file is where its identity belongs. The networked transports do not
    /// use this at all: they act as whoever called them.
    pub bot_token: Option<Secret>,
    pub workspace_id: Option<Uuid>,
    pub transport: McpTransport,
    pub bind_addr: Option<String>,
    /// Issuer that vouches for delegated tokens. All three `oauth_*` keys are set together or not
    /// at all; see [`McpConfig::runtime`].
    pub oauth_issuer: Option<String>,
    /// Identifier of this server as a protected resource (the URL that receives JSON-RPC posts).
    pub oauth_resource: Option<String>,
    /// Credential presented to the API when forwarding a delegated token. Never logged.
    pub oauth_forward_secret: Option<Secret>,
    /// Tool listing mode of the stdio transport and the default of the networked ones.
    pub tool_mode: McpToolMode,
    /// Whether `sylvode_write` may be used at all. `None` means the default, `true`.
    pub allow_write_call: Option<bool>,
    /// Whether the MCP server offers push (`subscriptions/listen`). `None` means the default, `true`.
    pub listen: Option<bool>,
    /// `Origin` values the MCP routes accept, exact `scheme://host[:port]`. `None` means none.
    pub allowed_origins: Option<Vec<String>>,
    /// Process wide cap on open listen streams of the HTTP transport. `None` means the default.
    pub max_listen_streams: Option<u32>,
    /// Peers whose `X-Real-IP` header names the real client. `None` means loopback only.
    pub trusted_proxies: Option<Vec<TrustedProxy>>,
}

/// A peer the MCP server believes about the client address: a reverse proxy in front of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedProxy {
    /// Any loopback address (`127.0.0.0/8`, `::1`).
    Loopback,
    /// Exactly this address.
    Address(std::net::IpAddr),
}

impl TrustedProxy {
    /// Parses one `[mcp] trusted_proxies` entry: `loopback` or an IP address.
    pub fn parse(entry: &str) -> Result<Self, String> {
        let entry = entry.trim();
        if entry.eq_ignore_ascii_case("loopback") {
            return Ok(Self::Loopback);
        }
        entry
            .parse()
            .map(Self::Address)
            .map_err(|_| format!("mcp.trusted_proxies entry {entry} must be `loopback` or an IP address"))
    }

    /// Whether `peer` is this proxy.
    pub fn matches(self, peer: std::net::IpAddr) -> bool {
        match self {
            Self::Loopback => peer.to_canonical().is_loopback(),
            Self::Address(address) => address.to_canonical() == peer.to_canonical(),
        }
    }
}

/// Default of `[mcp] max_listen_streams`.
pub const DEFAULT_MCP_MAX_LISTEN_STREAMS: u32 = 10_000;

/// What a deployment whose transport has no per-request credential is told.
///
/// `stdio` is a pipe pair and the CLI subcommands are a local process: neither carries a caller
/// credential, so both act as the configured bot and both need it to be there.
pub const MCP_BOT_TOKEN_REQUIRED: &str = "mcp.bot_token is required by the stdio transport and by the CLI subcommands, which have no \
     per-request credential to act on; it is not used, nor needed, when mcp.transport is http or \
     sse, because every networked call is made with the bot token its caller presented";

/// Why `mcp.workspace_id` is demanded of the `stdio` transport only.
const MCP_WORKSPACE_REQUIRED: &str = "mcp.workspace_id is required by the stdio transport, which has no per-request caller token to \
     take the workspace from; it is not used, nor needed, when mcp.transport is http or sse, because every networked call acts in \
     the workspace of the bot token its caller presented";

impl McpConfig {
    fn runtime(&self, origin: &Path) -> Result<McpRuntime, ConfigError> {
        let mut issues = Vec::new();
        // The networked transports authenticate every request against the caller's own
        // `Authorization: Bearer` bot token and forward it to the API, so they hold no
        // server-side identity at all and must not be forced to configure one.
        if !self.transport.is_networked() && self.bot_token.is_none() {
            issues.push(MCP_BOT_TOKEN_REQUIRED.to_string());
        }
        // The networked transports take the workspace from each caller's own bot token, so the
        // key is optional there; the pipe transport and the command line have no such token.
        if !self.transport.is_networked() && self.workspace_id.is_none() {
            issues.push(MCP_WORKSPACE_REQUIRED.to_string());
        }
        let oauth = self.oauth_runtime(&mut issues);
        if !issues.is_empty() {
            return Err(ConfigError::Invalid {
                path: origin.to_path_buf(),
                issues,
            });
        }
        Ok(McpRuntime {
            api_url: self.api_url.clone().unwrap_or_else(|| DEFAULT_MCP_API_URL.to_string()),
            bot_token: self.bot_token.clone(),
            workspace_id: self.workspace_id,
            transport: self.transport,
            bind_addr: self
                .bind_addr
                .clone()
                .unwrap_or_else(|| DEFAULT_MCP_BIND_ADDR.to_string()),
            oauth,
            tool_mode: self.tool_mode,
            allow_write_call: self.allow_write_call.unwrap_or(true),
            listen: self.listen.unwrap_or(true),
            allowed_origins: self.allowed_origins.clone().unwrap_or_default(),
            max_listen_streams: self.max_listen_streams.unwrap_or(DEFAULT_MCP_MAX_LISTEN_STREAMS),
            trusted_proxies: self
                .trusted_proxies
                .clone()
                .unwrap_or_else(|| vec![TrustedProxy::Loopback]),
        })
    }

    /// The three `oauth_*` keys are one unit: with none the server refuses delegated tokens, with
    /// all of them it validates them, and a partial set is refused by name instead of silently
    /// running without delegated access.
    fn oauth_runtime(&self, problems: &mut Vec<String>) -> Option<McpOAuthRuntime> {
        match (
            self.oauth_issuer.as_ref(),
            self.oauth_resource.as_ref(),
            self.oauth_forward_secret.as_ref(),
        ) {
            (Some(issuer_url), Some(resource_url), Some(forward_secret)) => Some(McpOAuthRuntime {
                issuer: issuer_url.clone(),
                resource: resource_url.clone(),
                forward_secret: forward_secret.clone(),
            }),
            (None, None, None) => None,
            (issuer_url, resource_url, secret) => {
                let missing: Vec<&str> = [
                    ("mcp.oauth_issuer", issuer_url.is_none()),
                    ("mcp.oauth_resource", resource_url.is_none()),
                    ("mcp.oauth_forward_secret", secret.is_none()),
                ]
                .into_iter()
                .filter_map(|(key, absent)| absent.then_some(key))
                .collect();
                problems.push(format!(
                    "mcp.oauth_issuer, mcp.oauth_resource and mcp.oauth_forward_secret are set together or not at \
                     all; missing: {}",
                    missing.join(", ")
                ));
                None
            }
        }
    }
}

/// The delegated-token settings of the MCP server, present only when all three keys are.
#[derive(Clone, Debug)]
pub struct McpOAuthRuntime {
    pub issuer: String,
    pub resource: String,
    pub forward_secret: Secret,
}

/// `[mcp]` with the mandatory fields resolved, as the MCP server needs them.
#[derive(Clone, Debug)]
pub struct McpRuntime {
    pub api_url: String,
    /// The configured identity. `None` is normal for an `http`/`sse` deployment, which never
    /// speaks to the API as itself; a command that does need one says so by name.
    pub bot_token: Option<Secret>,
    /// The configured workspace. Required by `stdio`; `http` and `sse` ignore it, because every
    /// networked call acts in the workspace its caller's bot token belongs to.
    pub workspace_id: Option<Uuid>,
    pub transport: McpTransport,
    pub bind_addr: String,
    /// `None` when `[mcp]` carries no `oauth_*` keys: delegated tokens are then refused.
    pub oauth: Option<McpOAuthRuntime>,
    /// `search` unless the file says `full`.
    pub tool_mode: McpToolMode,
    /// `true` unless the file says `false`; the runner writes `false` for its bots.
    pub allow_write_call: bool,
    /// `true` unless the file says `false`; `false` turns push off (`subscriptions/listen`).
    pub listen: bool,
    /// `Origin` values the MCP routes accept; empty by default.
    pub allowed_origins: Vec<String>,
    /// Open listen streams the HTTP transport accepts in total.
    pub max_listen_streams: u32,
    /// Peers whose `X-Real-IP` header names the real client; loopback by default.
    pub trusted_proxies: Vec<TrustedProxy>,
}

/// The subset of the configuration carried in `AppState` and reached from request handlers.
///
/// `Debug` is written by hand rather than derived so that adding a plain `String` credential to
/// this struct in the future cannot start printing it.
#[derive(Clone)]
pub struct AppConfig {
    pub app_name: String,
    pub bind_addr: String,
    pub database_url: Secret,
    pub jwt_secret: Secret,
    pub jwt_access_ttl_seconds: i64,
    pub jwt_refresh_ttl_seconds: i64,
    /// See [`AuthConfig::allow_insecure_cookies`]. Already validated against `bind_addr` by
    /// [`Self::from_config`]: by the time a handler reads this field, `true` implies `bind_addr`
    /// is a loopback host.
    pub allow_insecure_cookies: bool,
    /// See [`FlowConfig::collab_allowed_origins`]. Copied onto `AppConfig` (rather than read
    /// through `crate::config::runtime()` the way `dispatch_max_attempts` is) so every handler
    /// and test that already builds an `AppConfig` by hand can set it directly, per-case, instead
    /// of racing a process-wide `OnceLock` install.
    pub collab_allowed_origins: Vec<String>,
}

impl AppConfig {
    /// Projects a validated file onto the fields request handlers use.
    ///
    /// `default_name` and `default_bind` are the calling binary's own fallbacks, used when
    /// `[server]` does not name them — one file serves three binaries, so the file cannot carry a
    /// single correct answer for either.
    ///
    /// Fails when the file carries no database URL or no signing key. This is the API's and the
    /// worker's declaration that they need both, so it reports both at once rather than sending
    /// the operator round the loop twice.
    ///
    /// Also fails when `auth.allow_insecure_cookies = true` but the *resolved* `bind_addr`
    /// (file value, or `default_bind` when the file names none) is not a loopback host: skipping
    /// the `Secure` cookie attribute is only ever safe on a listener nothing off-machine can
    /// reach, and an operator who set the flag without also pinning the bind to loopback gets a
    /// startup failure instead of a cookie silently sent in the clear on a public interface.
    /// `site.cookie_insecure = true` follows the same rule.
    pub fn from_config(config: &OpenPrConfig, default_name: &str, default_bind: &str) -> Result<Self, ConfigError> {
        let (Some(database_url), Some(jwt_secret)) = (config.database.url.as_ref(), config.auth.jwt_secret.as_ref())
        else {
            let issues = [config.database.missing(), config.auth.missing()]
                .into_iter()
                .flatten()
                .map(str::to_owned)
                .collect();
            return Err(missing_values(&config.origin, issues));
        };

        let bind_addr = config
            .server
            .bind_addr
            .clone()
            .unwrap_or_else(|| default_bind.to_string());

        if config.auth.allow_insecure_cookies && !raw::is_loopback_bind_addr(&bind_addr) {
            return Err(missing_values(
                &config.origin,
                vec![format!(
                    "auth.allow_insecure_cookies=true requires server.bind_addr to resolve to a loopback host \
                     (127.0.0.1, ::1 or localhost); it resolves to '{bind_addr}'. Either bind to loopback for \
                     local development, or remove auth.allow_insecure_cookies so auth cookies stay Secure"
                )],
            ));
        }

        if config.site.cookie_insecure && !raw::is_loopback_bind_addr(&bind_addr) {
            return Err(missing_values(
                &config.origin,
                vec![format!(
                    "site.cookie_insecure=true requires server.bind_addr to resolve to a loopback host \
                     (127.0.0.1, ::1 or localhost) behind a local proxy; it resolves to '{bind_addr}'. Either \
                     bind to loopback, or remove site.cookie_insecure so site cookies stay Secure"
                )],
            ));
        }

        Ok(Self {
            app_name: config
                .server
                .app_name
                .clone()
                .unwrap_or_else(|| default_name.to_string()),
            bind_addr,
            database_url: database_url.clone(),
            jwt_secret: jwt_secret.clone(),
            jwt_access_ttl_seconds: config.auth.access_ttl_seconds,
            jwt_refresh_ttl_seconds: config.auth.refresh_ttl_seconds,
            allow_insecure_cookies: config.auth.allow_insecure_cookies,
            collab_allowed_origins: config.flow.collab_allowed_origins.clone(),
        })
    }
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfig")
            .field("app_name", &self.app_name)
            .field("bind_addr", &self.bind_addr)
            .field("database_url", &REDACTED)
            .field("jwt_secret", &REDACTED)
            .field("jwt_access_ttl_seconds", &self.jwt_access_ttl_seconds)
            .field("jwt_refresh_ttl_seconds", &self.jwt_refresh_ttl_seconds)
            .field("allow_insecure_cookies", &self.allow_insecure_cookies)
            .field("collab_allowed_origins", &self.collab_allowed_origins)
            .finish()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests;

#[cfg(test)]
#[allow(clippy::panic, clippy::pedantic, clippy::nursery)]
mod schema_tests;
