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
mod error;
mod inbound;
mod oauth;
mod raw;
mod secret;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use uuid::Uuid;

pub use cidr::IpCidr;
pub use error::{ConfigError, EXAMPLE_CONFIG_PATH};
pub use inbound::InboundConfig;
pub use oauth::{
    DEFAULT_ACCESS_TOKEN_TTL_SECONDS, DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS, DEFAULT_EVENT_BACKFILL_MAX_AGE_HOURS,
    DEFAULT_EVENT_BACKFILL_MAX_EVENTS, DEFAULT_EVENT_FEED_RETENTION_DAYS, DEFAULT_GRANT_MAX_LIFETIME_DAYS,
    DEFAULT_MAX_ACTIVE_GRANTS_PER_USER_CLIENT, DEFAULT_MAX_STREAMS_PER_PROCESS, DEFAULT_REFRESH_TOKEN_IDLE_DAYS,
    MIN_FORWARD_SECRET_LEN, OAuthConfig,
};
pub use secret::{REDACTED, Secret};

/// Where a binary looks for its configuration when no `--config` path is given.
///
/// Relative to the process working directory, so a container that mounts the file at
/// `/app/config/sylvode.toml` and runs with `/app` as its working directory needs no flag.
pub const DEFAULT_CONFIG_PATH: &str = "config/sylvode.toml";

/// v0.8 and older default. It is discovered only when the new default does not exist.
pub const LEGACY_CONFIG_PATH: &str = "config/openpr.toml";

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
/// Applies unless `[ai_tasks]` overrides it. Four times the runner's default agent run limit
/// (`sylvode-runner` `cli.timeout_secs = 900`), so a run that is merely slow, or waits for a free
/// executor slot, is not delivered a second time.
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
    pub ai_tasks: AiTasksConfig,
    pub migrations: MigrationsConfig,
    pub outbound: OutboundConfig,
    pub mcp: McpConfig,
    pub flow: FlowConfig,
    /// `None` unless `[oauth] issuer` is set: the authorization server is off by default.
    pub oauth: Option<OAuthConfig>,
    /// `[inbound]`: the alert chain's inbound endpoint, off unless `enabled = true`.
    pub inbound: InboundConfig,
}

impl OpenPrConfig {
    /// Reads and validates the configuration.
    ///
    /// `explicit` is the path a `--config` flag carried. When it is `None`,
    /// [`DEFAULT_CONFIG_PATH`] is used, relative to the process working directory.
    ///
    /// When `explicit` is `None` and only the legacy [`LEGACY_CONFIG_PATH`] exists, the legacy
    /// file is used and the process is marked so that [`take_legacy_discovery_notice`] returns
    /// the deprecation notice once. An explicit path is the operator's deliberate choice and is
    /// never marked, whatever it is called.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let (path, legacy_discovered) = resolve_config_path(explicit, Path::new("."))?;
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
        let config = Self::parse(&source, &absolute(&path))?;
        if legacy_discovered {
            LEGACY_DISCOVERED.store(true, Ordering::Relaxed);
        }
        Ok(config)
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

/// Whether this process loaded the legacy configuration file through default discovery.
static LEGACY_DISCOVERED: AtomicBool = AtomicBool::new(false);

/// Whether the legacy discovery notice has already been handed out in this process.
static LEGACY_NOTICE_TAKEN: AtomicBool = AtomicBool::new(false);

/// The ADR-0020 D2 notice for a legacy configuration file found by default discovery, the
/// first time it is asked for in a process that loaded one; `None` otherwise.
///
/// A binary calls this right after installing its logger and writes the result as one `warn`
/// line. Loading happens before the logger exists — the logger is configured by the file — so
/// the notice waits here rather than being emitted into a subscriber that is not there yet.
/// The swap makes "once per process" structural: a second caller gets `None`.
pub fn take_legacy_discovery_notice() -> Option<String> {
    if !LEGACY_DISCOVERED.load(Ordering::Relaxed) || LEGACY_NOTICE_TAKEN.swap(true, Ordering::Relaxed) {
        return None;
    }
    Some(crate::deprecation::legacy_config_discovery(
        LEGACY_CONFIG_PATH,
        DEFAULT_CONFIG_PATH,
    ))
}

/// The file to read, and whether it is the legacy file reached by default discovery.
fn resolve_config_path(explicit: Option<&Path>, base: &Path) -> Result<(PathBuf, bool), ConfigError> {
    if let Some(path) = explicit {
        return Ok((path.to_path_buf(), false));
    }
    let path = resolve_default_config_path(base)?;
    let legacy = path == base.join(LEGACY_CONFIG_PATH);
    Ok((path, legacy))
}

fn resolve_default_config_path(base: &Path) -> Result<PathBuf, ConfigError> {
    let canonical = base.join(DEFAULT_CONFIG_PATH);
    let legacy = base.join(LEGACY_CONFIG_PATH);
    match (canonical.exists(), legacy.exists()) {
        (true, true) => Err(ConfigError::Invalid {
            path: absolute(&canonical),
            issues: vec![format!(
                "both {DEFAULT_CONFIG_PATH} and legacy {LEGACY_CONFIG_PATH} exist; pass --config explicitly or remove one so configuration precedence is never silent"
            )],
        }),
        (false, true) => Ok(legacy),
        (_, false) => Ok(canonical),
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
    /// Reverse proxies whose `X-Forwarded-For` / `X-Real-IP` headers may be believed.
    /// `None` means the key was absent and the API applies its built-in default.
    pub trusted_proxies: Option<Vec<IpCidr>>,
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
    pub default_author_id: Option<Uuid>,
    /// Explicit developer opt-out of the `Secure` attribute on the access/refresh/clear auth
    /// cookies. Defaults to `false` (cookies are always `Secure`). `AppConfig::from_config`
    /// refuses to start when this is `true` but `server.bind_addr` does not resolve to a loopback
    /// host: skipping `Secure` is only safe when the listener itself is unreachable from outside
    /// the machine, and a missing/non-loopback bind must fail closed rather than silently ship an
    /// insecure cookie to the network.
    pub allow_insecure_cookies: bool,
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
            default_author_id: self.default_author_id,
        })
    }
}

/// `[auth]` with the mandatory fields resolved, as a binary that issues tokens needs them.
#[derive(Clone, Debug)]
pub struct AuthRuntime {
    pub jwt_secret: Secret,
    pub access_ttl_seconds: i64,
    pub refresh_ttl_seconds: i64,
    pub default_author_id: Option<Uuid>,
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

/// `[ai_tasks]` — the worker's handling of AI tasks it has delivered.
///
/// `processing_timeout_secs` bounds how long a delivered task may stay `processing` without the
/// receiver completing or failing it. After that the worker ends the attempt: the task goes back
/// to `pending` while attempts remain and is failed otherwise.
///
/// `lease_extension_secs` is how far an acknowledge, a progress report or a runner heartbeat moves
/// the lease of an active task; `max_processing_secs` is the hard cap from `started_at` that no
/// extension passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiTasksConfig {
    pub processing_timeout_secs: u64,
    pub lease_extension_secs: u64,
    pub max_processing_secs: u64,
}

impl Default for AiTasksConfig {
    fn default() -> Self {
        Self {
            processing_timeout_secs: DEFAULT_AI_TASK_PROCESSING_TIMEOUT_SECS,
            lease_extension_secs: DEFAULT_AI_TASK_LEASE_EXTENSION_SECS,
            max_processing_secs: DEFAULT_AI_TASK_MAX_PROCESSING_SECS,
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

/// `[migrations]` — the two escape hatches of the migration runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct MigrationsConfig {
    /// Re-execute every migration once, reporting failures without aborting.
    pub replay: bool,
    /// Start even though a migration failed or the schema check found a gap.
    pub continue_on_error: bool,
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
}

/// What a deployment whose transport has no per-request credential is told.
///
/// `stdio` is a pipe pair and the CLI subcommands are a local process: neither carries a caller
/// credential, so both act as the configured bot and both need it to be there.
pub const MCP_BOT_TOKEN_REQUIRED: &str = "mcp.bot_token is required by the stdio transport and by the CLI subcommands, which have no \
     per-request credential to act on; it is not used, nor needed, when mcp.transport is http or \
     sse, because every networked call is made with the bot token its caller presented";

impl McpConfig {
    fn runtime(&self, origin: &Path) -> Result<McpRuntime, ConfigError> {
        let mut issues = Vec::new();
        // The networked transports authenticate every request against the caller's own
        // `Authorization: Bearer` bot token and forward it to the API, so they hold no
        // server-side identity at all and must not be forced to configure one.
        if !self.transport.is_networked() && self.bot_token.is_none() {
            issues.push(MCP_BOT_TOKEN_REQUIRED.to_string());
        }
        if self.workspace_id.is_none() {
            issues.push("mcp.workspace_id is required to run the MCP server".to_string());
        }
        let oauth = self.oauth_runtime(&mut issues);
        let Some(workspace_id) = self.workspace_id.filter(|_| issues.is_empty()) else {
            return Err(ConfigError::Invalid {
                path: origin.to_path_buf(),
                issues,
            });
        };
        Ok(McpRuntime {
            api_url: self.api_url.clone().unwrap_or_else(|| DEFAULT_MCP_API_URL.to_string()),
            bot_token: self.bot_token.clone(),
            workspace_id,
            transport: self.transport,
            bind_addr: self
                .bind_addr
                .clone()
                .unwrap_or_else(|| DEFAULT_MCP_BIND_ADDR.to_string()),
            oauth,
            tool_mode: self.tool_mode,
            allow_write_call: self.allow_write_call.unwrap_or(true),
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
    pub workspace_id: Uuid,
    pub transport: McpTransport,
    pub bind_addr: String,
    /// `None` when `[mcp]` carries no `oauth_*` keys: delegated tokens are then refused.
    pub oauth: Option<McpOAuthRuntime>,
    /// `search` unless the file says `full`.
    pub tool_mode: McpToolMode,
    /// `true` unless the file says `false`; the runner writes `false` for its bots.
    pub allow_write_call: bool,
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
    pub default_author_id: Option<Uuid>,
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
            default_author_id: config.auth.default_author_id,
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
            .field("default_author_id", &self.default_author_id)
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
