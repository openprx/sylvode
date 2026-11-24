use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub features: FeatureConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    /// Terminal callbacks for worker AI tasks (`[ai_task_callback]`).
    #[serde(default)]
    pub ai_task_callback: AiTaskCallbackConfig,
    /// The `runner` agents' settings (`[runner]`).
    #[serde(default)]
    pub runner: crate::runner::config::RunnerConfig,
    /// Where the file was read from (canonical): the file holds every token, so no project may
    /// lie over it and the CLI may not read it. Not read from the file.
    #[serde(skip)]
    #[allow(clippy::struct_field_names)]
    pub config_path: Option<std::path::PathBuf>,
    /// Safe mode, resolved once at start-up from `SYLVODE_RUNNER_SAFE_MODE`. Not read from the file.
    #[serde(skip)]
    pub safe_mode: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct FeatureConfig {
    #[serde(default)]
    pub tunnel_enabled: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    #[serde(default = "default_cli_max_concurrency")]
    pub cli_max_concurrency: usize,
    #[serde(default = "default_http_timeout_secs")]
    pub http_timeout_secs: u64,
    #[serde(default = "default_tunnel_reconnect_backoff_max_secs")]
    pub tunnel_reconnect_backoff_max_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            cli_max_concurrency: default_cli_max_concurrency(),
            http_timeout_secs: default_http_timeout_secs(),
            tunnel_reconnect_backoff_max_secs: default_tunnel_reconnect_backoff_max_secs(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub listen: String,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    #[serde(default)]
    pub webhook_secrets: Vec<String>,
    #[serde(default)]
    pub allow_unsigned: bool,
    /// Acknowledges that `allow_unsigned = true` is meant on an address other than loopback, where
    /// anyone who can reach the port can trigger agents. Without it that combination is refused.
    #[serde(default)]
    pub allow_unsigned_non_loopback: bool,
    /// How far a signed delivery's `timestamp` may be from now, in seconds, before it is refused
    /// as a replay. `0` turns the check off.
    #[serde(default = "default_max_delivery_age_secs")]
    pub max_delivery_age_secs: u64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub agent_type: String,
    #[serde(default)]
    pub route: Option<RouteConfig>,
    pub openclaw: Option<OpenClawConfig>,
    pub openprx: Option<OpenPRXConfig>,
    pub webhook: Option<WebhookAgentConfig>,
    pub custom: Option<CustomConfig>,
    /// The roots this `runner` agent's projects may lie under. Without it the agent uses
    /// `[runner] allowed_roots`; with more than one `runner` agent every agent needs its own, and
    /// they must not overlap: one agent's task can then never read another agent's tree.
    #[serde(default)]
    pub allowed_roots: Vec<String>,
    pub message_template: Option<String>,
}

impl AgentConfig {
    /// The bots a `runner` agent serves: `route.bot_ids`, else the agent id.
    pub fn runner_bot_ids(&self) -> Vec<String> {
        self.route
            .as_ref()
            .filter(|route| !route.bot_ids.is_empty())
            .map_or_else(|| vec![self.id.clone()], |route| route.bot_ids.clone())
    }
}

/// The bots' tokens for the Sylvode API (`[ai_task_callback]`).
///
/// The runner reads and reports tasks with them (`runner` agents), and a forwarding agent
/// (`webhook`, `openclaw`, `openprx`, `custom`) that cannot forward a task ends it through
/// `POST /api/v1/ai/tasks/{task_id}/fail` with the task's bot token.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AiTaskCallbackConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Base URL of the Sylvode API, for example `http://127.0.0.1:8080`. Required when enabled.
    pub api_url: Option<String>,
    /// Bearer token: the `opr_` token of the bot that is the task's AI participant (the API only
    /// lets the assigned participant, or a system administrator, end a task). It works only for
    /// the tasks of the bot that owns it. An agent can override it with
    /// `[agents.ai_task_callback].token`, and `bots` gives other bots their own token.
    pub token: Option<String>,
    /// The id of the bot that owns `token`. When set, `token` is used only for that bot's tasks;
    /// when not set, it is used for every task no other token covers (and the API refuses it for
    /// the tasks of any other bot).
    pub bot_id: Option<String>,
    /// Tokens of further bots, keyed by bot id (`[[ai_task_callback.bots]]`): used for the tasks
    /// of that bot that no agent token covers, including tasks no agent matches.
    #[serde(default)]
    pub bots: Vec<AiTaskCallbackBotConfig>,
    /// The transport the token is registered with in Sylvode (`workspace_bots.transport_surface`).
    /// `rest` sends no surface header; any other value is sent as `X-Sylvode-MCP-Surface`.
    #[serde(default = "default_ai_task_surface")]
    pub surface: String,
    /// Attempts per callback, including the first one (1 to 10).
    #[serde(default = "default_ai_task_max_attempts")]
    pub max_attempts: u32,
    /// Delay before the first retry, doubled after each further failed attempt (up to 60000).
    #[serde(default = "default_ai_task_retry_delay_ms")]
    pub retry_delay_ms: u64,
    /// Acknowledges an `http://` `api_url` to a host other than loopback, which sends the bot
    /// tokens and every task in clear text unless the network itself is encrypted (for example a
    /// `WireGuard` intranet). Without it that is refused for a `runner` agent.
    #[serde(default)]
    pub allow_insecure_transport: bool,
}

impl Default for AiTaskCallbackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: None,
            token: None,
            bot_id: None,
            bots: Vec::new(),
            surface: default_ai_task_surface(),
            max_attempts: default_ai_task_max_attempts(),
            retry_delay_ms: default_ai_task_retry_delay_ms(),
            allow_insecure_transport: false,
        }
    }
}

/// The callback token of one bot (`[[ai_task_callback.bots]]`).
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AiTaskCallbackBotConfig {
    /// The bot's user id in Sylvode (a UUID), as the worker sends it in `ai_participant_id`.
    pub bot_id: String,
    /// The workspace the bot belongs to (a UUID). A bot lives in exactly one workspace; the MCP
    /// server the runner starts for the CLI needs it, so a runner bot's entry must carry it.
    pub workspace_id: Option<String>,
    /// The bot's `opr_` token.
    pub token: String,
    /// The transport the token is registered with; defaults to `ai_task_callback.surface`.
    pub surface: Option<String>,
    /// The secret that signs this bot's deliveries. A delivery for the bot is verified with this
    /// secret only (and a delivery for another bot never with it). Without it the bot's deliveries
    /// are verified with `security.webhook_secrets`, which must then belong to this bot alone.
    pub webhook_secret: Option<String>,
}

/// Shows that a token is set without showing it.
const fn redacted(token: Option<&str>) -> &'static str {
    match token {
        Some(_) => "<redacted>",
        None => "<none>",
    }
}

impl std::fmt::Debug for AiTaskCallbackConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiTaskCallbackConfig")
            .field("enabled", &self.enabled)
            .field("api_url", &self.api_url)
            .field("token", &redacted(self.token.as_deref()))
            .field("bot_id", &self.bot_id)
            .field("bots", &self.bots)
            .field("surface", &self.surface)
            .field("max_attempts", &self.max_attempts)
            .field("retry_delay_ms", &self.retry_delay_ms)
            .field("allow_insecure_transport", &self.allow_insecure_transport)
            .finish()
    }
}

impl std::fmt::Debug for AiTaskCallbackBotConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiTaskCallbackBotConfig")
            .field("bot_id", &self.bot_id)
            .field("workspace_id", &self.workspace_id)
            .field("token", &"<redacted>")
            .field("surface", &self.surface)
            .field("webhook_secret", &redacted(self.webhook_secret.as_deref()))
            .finish()
    }
}

impl std::fmt::Debug for SecurityConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityConfig")
            .field(
                "webhook_secrets",
                &format_args!("<{} redacted>", self.webhook_secrets.len()),
            )
            .field("allow_unsigned", &self.allow_unsigned)
            .field("allow_unsigned_non_loopback", &self.allow_unsigned_non_loopback)
            .field("max_delivery_age_secs", &self.max_delivery_age_secs)
            .finish()
    }
}

impl std::fmt::Debug for WebhookAgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookAgentConfig")
            .field("url", &self.url)
            .field("secret", &redacted(self.secret.as_deref()))
            .finish()
    }
}

impl std::fmt::Debug for TunnelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelConfig")
            .field("enabled", &self.enabled)
            .field("url", &self.url)
            .field("agent_id", &self.agent_id)
            .field("auth_token", &redacted(self.auth_token.as_deref()))
            .field("reconnect_secs", &self.reconnect_secs)
            .field("heartbeat_secs", &self.heartbeat_secs)
            .field("hmac_secret", &redacted(self.hmac_secret.as_deref()))
            .field("require_inbound_sig", &self.require_inbound_sig)
            .field("allow_insecure_transport", &self.allow_insecure_transport)
            .finish()
    }
}

/// The transports a Sylvode bot token can be registered with (`EventSurface::BOT_CREDENTIAL_SURFACES`
/// in the Sylvode API).
pub const AI_TASK_SURFACES: [&str; 6] = ["rest", "mcp_http", "mcp_sse", "mcp_stdio", "cli", "cli_tools_call"];

/// How one agent reports AI task outcomes, resolved from the global and per-agent settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiTaskCallbackTarget<'a> {
    pub api_url: &'a str,
    pub token: &'a str,
    pub surface: &'a str,
    /// The bot's workspace, from its `[[ai_task_callback.bots]]` entry.
    pub workspace_id: Option<&'a str>,
}

/// Whether and how the outcome of one worker AI task is reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiTaskCallback<'a> {
    /// The callback does not apply: it is disabled, safe mode is on, or the agent opts out.
    Off,
    /// Report with this target.
    Report(AiTaskCallbackTarget<'a>),
    /// The callback applies, but no configured token belongs to the task's bot. Calling the API
    /// with another bot's token would only be refused, so nothing is called.
    NoToken,
}

impl AiTaskCallback<'_> {
    /// The value of `ai_task_callback` in the `/webhook` answer to a worker AI task.
    pub const fn answer(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Report(_) => "report",
            Self::NoToken => "no_token",
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    #[serde(default)]
    pub bot_names: Vec<String>,
    #[serde(default)]
    pub bot_ids: Vec<String>,
    #[serde(default)]
    pub bot_agent_types: Vec<String>,
    #[serde(default)]
    pub project_types: Vec<String>,
    #[serde(default)]
    pub trigger_kinds: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub form_keys: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct OpenClawConfig {
    pub command: String,
    /// Fixed arguments placed between `command` and `--channel`.
    #[serde(default)]
    pub args: Vec<String>,
    pub channel: String,
    pub target: String,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WebhookAgentConfig {
    pub url: String,
    pub secret: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct OpenPRXConfig {
    /// Signal daemon HTTP API base URL (e.g. `<http://127.0.0.1:8686>`)
    pub signal_api: Option<String>,
    /// Target recipient (phone number or uuid)
    pub target: String,
    /// Account phone number for signal-cli
    pub account: Option<String>,
    /// Or send through a command (e.g. `openprx`, with `args = ["message", "send"]`)
    pub command: Option<String>,
    /// Fixed arguments placed between `command` and `--channel`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Channel name (signal, wacli, etc.)
    #[serde(default = "default_channel")]
    pub channel: String,
}

fn default_channel() -> String {
    "signal".into()
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CustomConfig {
    pub command: String,
    pub args: Option<Vec<String>>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TunnelConfig {
    #[serde(default)]
    pub enabled: bool,
    pub url: Option<String>,
    pub agent_id: Option<String>,
    pub auth_token: Option<String>,
    #[serde(default = "default_reconnect_secs")]
    pub reconnect_secs: u64,
    #[serde(default = "default_heartbeat_secs")]
    pub heartbeat_secs: u64,
    pub hmac_secret: Option<String>,
    #[serde(default)]
    pub require_inbound_sig: bool,
    /// Acknowledges a `ws://` (unencrypted) `url` to a host other than loopback, which sends the
    /// bearer token and every task in clear text. Without it that is refused.
    #[serde(default)]
    pub allow_insecure_transport: bool,
}

const fn default_max_delivery_age_secs() -> u64 {
    300
}

const fn default_reconnect_secs() -> u64 {
    3
}

const fn default_heartbeat_secs() -> u64 {
    20
}

const fn default_cli_max_concurrency() -> usize {
    1
}

const fn default_http_timeout_secs() -> u64 {
    15
}

const fn default_tunnel_reconnect_backoff_max_secs() -> u64 {
    60
}

fn default_ai_task_surface() -> String {
    "rest".into()
}

const fn default_ai_task_max_attempts() -> u32 {
    3
}

const fn default_ai_task_retry_delay_ms() -> u64 {
    1000
}

/// Upper bound of `ai_task_callback.max_attempts`.
const MAX_AI_TASK_ATTEMPTS: u32 = 10;
/// Upper bound of `ai_task_callback.retry_delay_ms`.
const MAX_AI_TASK_RETRY_DELAY_MS: u64 = 60_000;

/// The secret `config.example.toml` ships with. Start-up is refused while it is still in place.
pub const EXAMPLE_SECRET_PLACEHOLDER: &str = "replace-with-the-secret-of-your-sylvode-runner";

/// Secrets copied from an example file rather than chosen: the current example's placeholder, the
/// values earlier examples and the README showed, and the placeholder of the compose example in
/// the Sylvode repository (`config/sylvode-runner.example.toml`).
pub const PLACEHOLDER_SECRETS: [&str; 5] = [
    EXAMPLE_SECRET_PLACEHOLDER,
    "your-secret-here",
    "shared-hmac-secret",
    "replace-with-the-secret-shared-with-the-control-plane",
    "replace_with_webhook_secret",
];

/// Text that marks a value as a placeholder someone forgot to replace, wherever it appears.
const PLACEHOLDER_MARKERS: [&str; 6] = ["replace_with", "replace-with", "your_", "your-", "changeme", "example"];

/// Whether `secret`, ignoring surrounding whitespace and case, is one of [`PLACEHOLDER_SECRETS`]
/// or contains one of [`PLACEHOLDER_MARKERS`].
fn is_placeholder(secret: &str) -> bool {
    let secret = secret.trim().to_ascii_lowercase();
    PLACEHOLDER_SECRETS.contains(&secret.as_str()) || PLACEHOLDER_MARKERS.iter().any(|marker| secret.contains(marker))
}

/// Whether `url` names a loopback host exactly: `localhost` (any case), an address in
/// `127.0.0.0/8`, or `::1`, with no user information. The host is taken from the parsed URL, so
/// `localhost.evil`, `localhost:x@evil.com` (whose host is `evil.com`) and a bracketed host other
/// than `::1` do not count; an IPv4-mapped IPv6 address does not either.
fn is_loopback_url(url: &url::Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Whether `listen` (`host:port`) binds only a loopback interface. A host name other than
/// `localhost` may resolve to anything, so it does not count as loopback.
fn is_loopback_listen(listen: &str) -> bool {
    url::Url::parse(&format!("http://{listen}/")).is_ok_and(|url| {
        url.port().is_some()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && is_loopback_url(&url)
    })
}

/// Whether `url` is a `ws://` URL whose host is not loopback. A `ws://` URL that does not parse is
/// treated as remote.
fn is_remote_plain_ws(url: &str) -> bool {
    url::Url::parse(url).map_or_else(
        |_| url.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("ws://")),
        |parsed| parsed.scheme() == "ws" && !is_loopback_url(&parsed),
    )
}

/// Whether `bot_id` (a configured id) names the task bot `task_bot` (the delivery's
/// `ai_participant_id`): both are UUIDs and equal, whatever their case.
pub fn same_bot(bot_id: &str, task_bot: &str) -> bool {
    match (
        uuid::Uuid::parse_str(bot_id.trim()),
        uuid::Uuid::parse_str(task_bot.trim()),
    ) {
        (Ok(configured), Ok(task)) => configured == task,
        _ => false,
    }
}

/// Whether `token` is filled in: not blank and not a placeholder.
fn usable_token(token: &str) -> bool {
    !token.trim().is_empty() && !is_placeholder(token)
}

/// Refuses an `ai_task_callback.api_url` that is not an absolute `http(s)` URL without
/// credentials, a query or a fragment (the endpoint paths are appended to it).
fn validate_api_url(api_url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(api_url.trim()).map_err(|e| {
        format!(
            "ai_task_callback.api_url must be the base URL of the Sylvode API (for example http://127.0.0.1:8080): {e}"
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("ai_task_callback.api_url must use http:// or https://".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("ai_task_callback.api_url must not carry credentials; use ai_task_callback.token".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("ai_task_callback.api_url must not have a query or a fragment".to_string());
    }
    Ok(())
}

/// The agent types the runner dispatches to.
pub const AGENT_TYPES: [&str; 5] = ["runner", "openclaw", "openprx", "webhook", "custom"];

/// Every trigger kind a Sylvode deployment sends, the closed set `route.trigger_kinds` may name.
///
/// A direct webhook delivery carries `bot_context.trigger_reason`: `created`, `assigned`,
/// `status_changed` or `mentioned`. A worker-dispatched AI task carries a
/// top-level `trigger_kind`: `assigned`, `mention`, `proposal_vote` or `manual`.
pub const TRIGGER_KINDS: [&str; 7] = [
    "assigned",
    "created",
    "status_changed",
    "mentioned",
    "mention",
    "proposal_vote",
    "manual",
];

/// Refuses a command that is not one program name. Commands run without a shell, so a value
/// with whitespace in it (a program followed by arguments) is never split: the arguments belong
/// in `args`.
fn check_single_program(agent_id: &str, key: &str, command: &str) -> Result<(), String> {
    if command.contains(char::is_whitespace) {
        return Err(format!(
            "agent `{agent_id}`: {key} must name one program; put its arguments in {}",
            key.replace("command", "args")
        ));
    }
    Ok(())
}

impl Config {
    /// The `runner` agent that serves `bot_id`, if any.
    pub fn runner_agent_for_bot(&self, bot_id: &str) -> Option<&AgentConfig> {
        self.agents
            .iter()
            .filter(|agent| agent.agent_type == "runner")
            .find(|agent| agent.runner_bot_ids().iter().any(|id| same_bot(id, bot_id)))
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("failed to read config {path}: {e}"))?;
        let mut config = Self::from_toml(&content)?;
        config.config_path = std::fs::canonicalize(path).ok();
        Ok(config)
    }

    /// Parses and validates a configuration.
    pub fn from_toml(content: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(content).map_err(|e| format!("failed to parse config: {e}"))?;
        config.validate_commands()?;
        config.validate()?;
        config.validate_runner_agents()?;
        Ok(config)
    }

    /// Refuses a `runner` agent that cannot work, and an invalid `[runner]` block.
    ///
    /// A `runner` agent needs `[ai_task_callback] enabled = true` and, for
    /// every bot of its route a token registered for `mcp_stdio` (design C17), and takes no agent
    /// type table: its permissions come from `[runner.cli.*]` and its directory from Sylvode.
    fn validate_runner_agents(&self) -> Result<(), String> {
        self.runner.validate()?;
        let runners: Vec<&AgentConfig> = self.agents.iter().filter(|a| a.agent_type == "runner").collect();
        if runners.is_empty() {
            return Ok(());
        }
        let cfg = &self.ai_task_callback;
        // The bearer token goes to this URL: in clear text only to a loopback host, unless the
        // operator acknowledges that the network path is encrypted.
        if cfg.enabled && !cfg.allow_insecure_transport && self.api_url_is_remote_plain_http() {
            return Err(
                "ai_task_callback.api_url uses http:// to a host other than loopback, which would send bot \
                 tokens in clear text; use https://, or set ai_task_callback.allow_insecure_transport = true \
                 if the network path is encrypted (for example a WireGuard intranet)"
                    .to_string(),
            );
        }
        for agent in &runners {
            let id = &agent.id;
            if agent.openclaw.is_some() || agent.openprx.is_some() || agent.webhook.is_some() || agent.custom.is_some()
            {
                return Err(format!(
                    "agent `{id}`: a runner agent takes no [agents.custom] or other agent type \
                     table; its permissions come from [runner.cli.*]"
                ));
            }
            if !cfg.enabled {
                return Err(format!(
                    "agent `{id}`: a runner agent needs [ai_task_callback] enabled = true: it reads and \
                     reports tasks with the bot's token"
                ));
            }
            if runners.len() > 1 && agent.allowed_roots.is_empty() {
                return Err(format!(
                    "agent `{id}`: with more than one runner agent each needs its own allowed_roots (disjoint \
                     from the others'): one agent's tasks must not be able to read another's tree"
                ));
            }
            for bot in agent.runner_bot_ids() {
                if uuid::Uuid::parse_str(bot.trim()).is_err() {
                    return Err(format!(
                        "agent `{id}`: a runner agent serves bots by id; set route.bot_ids (or make the agent id \
                         the bot's UUID)"
                    ));
                }
                let surface = if let Some(entry) = cfg.bots.iter().find(|b| same_bot(&b.bot_id, &bot)) {
                    let workspace = entry.workspace_id.as_deref().map(str::trim).unwrap_or_default();
                    match uuid::Uuid::parse_str(workspace) {
                        Ok(parsed) if !parsed.is_nil() => {}
                        _ => {
                            return Err(format!(
                                "bot {bot}: a runner bot's [[ai_task_callback.bots]] entry needs workspace_id, the \
                                 UUID of the workspace the bot belongs to: the MCP server the runner starts for the \
                                 CLI cannot start without it"
                            ));
                        }
                    }
                    entry.surface.as_deref().unwrap_or(cfg.surface.as_str())
                } else {
                    return Err(format!(
                        "agent `{id}`: bot {bot} has no token in [ai_task_callback]; add a \
                         [[ai_task_callback.bots]] entry with surface = \"mcp_stdio\""
                    ));
                };
                if surface != "mcp_stdio" {
                    return Err(format!("bot {bot}: runner bots need a token registered for mcp_stdio"));
                }
            }
        }
        // More than one bot on one runner: each bot's deliveries are signed with its own secret.
        let runner_bots: Vec<String> = runners.iter().flat_map(|a| a.runner_bot_ids()).collect();
        if runner_bots.len() > 1 {
            for bot in &runner_bots {
                let bound = cfg
                    .bots
                    .iter()
                    .find(|entry| same_bot(&entry.bot_id, bot))
                    .and_then(|entry| entry.webhook_secret.as_deref());
                if !bound.is_some_and(|secret| !secret.trim().is_empty() && !is_placeholder(secret)) {
                    return Err(format!(
                        "bot {bot}: a runner that serves more than one bot needs a webhook_secret on every \
                         [[ai_task_callback.bots]] entry, so one bot's secret cannot sign another bot's deliveries"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Refuses a `command` that is more than one program name; see [`check_single_program`].
    fn validate_commands(&self) -> Result<(), String> {
        for agent in &self.agents {
            let id = agent.id.as_str();
            if let Some(cfg) = agent.openclaw.as_ref() {
                check_single_program(id, "openclaw.command", &cfg.command)?;
            }
            if let Some(command) = agent.openprx.as_ref().and_then(|cfg| cfg.command.as_deref()) {
                check_single_program(id, "openprx.command", command)?;
            }
            if let Some(cfg) = agent.custom.as_ref() {
                check_single_program(id, "custom.command", &cfg.command)?;
            }
        }
        Ok(())
    }

    /// Refuses settings that parse but can never work, so they fail at start-up instead of
    /// silently never dispatching.
    fn validate(&self) -> Result<(), String> {
        self.validate_secrets()?;
        self.validate_tunnel()?;
        self.validate_ai_task_callback()?;
        for agent in &self.agents {
            if !AGENT_TYPES.contains(&agent.agent_type.as_str()) {
                return Err(format!(
                    "agent `{}`: unknown agent_type `{}`; valid values: {}",
                    agent.id,
                    agent.agent_type,
                    AGENT_TYPES.join(", ")
                ));
            }
            if let Some(route) = &agent.route {
                for kind in &route.trigger_kinds {
                    let known = TRIGGER_KINDS
                        .iter()
                        .any(|valid| valid.eq_ignore_ascii_case(kind.trim()));
                    if !known {
                        return Err(format!(
                            "agent `{}`: route.trigger_kinds contains `{kind}`, which Sylvode never sends; \
                             valid values: {}",
                            agent.id,
                            TRIGGER_KINDS.join(", ")
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The secrets a delivery that claims to be for `bot` may be signed with: that bot's own
    /// `webhook_secret` when it has one (and no other), else `security.webhook_secrets`. A secret
    /// bound to one bot never verifies a delivery for another.
    pub fn webhook_secrets_for(&self, bot: Option<&str>) -> Vec<String> {
        let bound = bot.and_then(|bot| {
            self.ai_task_callback
                .bots
                .iter()
                .find(|entry| same_bot(&entry.bot_id, bot))
                .and_then(|entry| entry.webhook_secret.clone())
        });
        bound.map_or_else(|| self.security.webhook_secrets.clone(), |secret| vec![secret])
    }

    /// Whether `[tunnel]` is in use: enabled by both switches. Safe mode is applied later and only
    /// turns a configured tunnel off for one run, so it does not count here.
    fn tunnel_in_use(&self) -> bool {
        self.features.tunnel_enabled && self.tunnel.as_ref().is_some_and(|t| t.enabled)
    }

    /// Problems with the secrets: a blank one (an HMAC with an empty key is one anyone can
    /// compute, so it would turn signature checking into no check) or one still set to an example
    /// placeholder. Each comes with whether the secret is in use: `webhook_secrets` unless
    /// `allow_unsigned`, `hmac_secret` when the tunnel is in use. Those in use refuse start-up; the
    /// others are only warned about, so a setting left over beside a disabled feature (such as
    /// the `hmac_secret = "shared-hmac-secret"` earlier examples shipped) does not stop an upgrade.
    fn secret_problems(&self) -> Vec<(bool, String)> {
        let mut problems = Vec::new();
        let webhook_in_use = !self.security.allow_unsigned;
        for (index, secret) in self.security.webhook_secrets.iter().enumerate() {
            if is_placeholder(secret) {
                problems.push((
                    webhook_in_use,
                    format!(
                        "security.webhook_secrets[{index}] is still the example placeholder; set it to the secret \
                         configured on the Sylvode webhook (a long random string)"
                    ),
                ));
            } else if secret.trim().is_empty() {
                problems.push((
                    webhook_in_use,
                    format!(
                        "security.webhook_secrets[{index}] is blank; an empty HMAC key lets anyone sign a request. \
                         Set it to the secret configured on the Sylvode webhook, or remove the entry"
                    ),
                ));
            }
        }
        if let Some(secret) = self.tunnel.as_ref().and_then(|t| t.hmac_secret.as_deref()) {
            let in_use = self.tunnel_in_use();
            if is_placeholder(secret) {
                problems.push((
                    in_use,
                    "tunnel.hmac_secret is still an example placeholder; set it to the secret shared with \
                     the control plane, or remove the setting"
                        .to_string(),
                ));
            } else if secret.trim().is_empty() {
                problems.push((
                    in_use,
                    "tunnel.hmac_secret is blank; an empty HMAC key lets anyone sign an envelope. Set it \
                     to the secret shared with the control plane, or remove the setting"
                        .to_string(),
                ));
            }
        }
        problems
    }

    /// Refuses a blank or placeholder secret that is in use, and unsigned requests on an address
    /// other than loopback.
    fn validate_secrets(&self) -> Result<(), String> {
        if let Some((_, problem)) = self.secret_problems().into_iter().find(|(in_use, _)| *in_use) {
            return Err(problem);
        }
        if self.security.allow_unsigned
            && !self.security.allow_unsigned_non_loopback
            && !is_loopback_listen(&self.server.listen)
        {
            return Err(format!(
                "security.allow_unsigned = true with server.listen = \"{}\", which is not a loopback address: \
                 anyone who can reach the port could trigger agents without a signature. Listen on \
                 127.0.0.1, set allow_unsigned = false, or, if this is really intended, set \
                 security.allow_unsigned_non_loopback = true",
                self.server.listen
            ));
        }
        Ok(())
    }

    /// Refuses `require_inbound_sig` on an enabled tunnel without `hmac_secret`.
    ///
    /// Without a secret the `sig` of an inbound envelope is never verified, so requiring one would
    /// only check that the field exists and would authenticate nothing.
    fn validate_tunnel(&self) -> Result<(), String> {
        let Some(tunnel) = self.tunnel.as_ref().filter(|_| self.tunnel_in_use()) else {
            return Ok(());
        };
        if let Some(url) = &tunnel.url
            && !tunnel.allow_insecure_transport
            && is_remote_plain_ws(url)
        {
            return Err(format!(
                "tunnel.url = \"{url}\" uses unencrypted ws:// to a host other than loopback, which sends the \
                 bearer token and every task in clear text; use wss://, or set \
                 tunnel.allow_insecure_transport = true if the network path is trusted"
            ));
        }
        let has_secret = tunnel.hmac_secret.as_deref().is_some_and(|s| !s.trim().is_empty());
        if tunnel.require_inbound_sig && !has_secret {
            return Err(
                "tunnel.require_inbound_sig = true requires a non-empty tunnel.hmac_secret: without it \
                        the sig of inbound envelopes cannot be verified and authenticates nothing; set \
                        tunnel.hmac_secret to the secret shared with the control plane"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Refuses an `[ai_task_callback]` that is enabled but cannot work: no usable API URL, a
    /// surface the API does not know, out-of-range retry settings, or an agent that would report
    /// without a token.
    fn validate_ai_task_callback(&self) -> Result<(), String> {
        let cfg = &self.ai_task_callback;
        let surfaces = std::iter::once(("ai_task_callback.surface".to_string(), cfg.surface.as_str()));
        let bot_surfaces = cfg.bots.iter().filter_map(|bot| {
            Some((
                format!("ai_task_callback.bots (bot {}): surface", bot.bot_id.trim()),
                bot.surface.as_deref()?,
            ))
        });
        for (key, surface) in surfaces.chain(bot_surfaces) {
            if !AI_TASK_SURFACES.contains(&surface) {
                return Err(format!(
                    "{key} = \"{surface}\" is not a transport a Sylvode bot token can be registered with; \
                     valid values: {}",
                    AI_TASK_SURFACES.join(", ")
                ));
            }
        }
        let bot_ids = cfg
            .bot_id
            .iter()
            .map(|id| ("ai_task_callback.bot_id", id))
            .chain(cfg.bots.iter().map(|bot| ("ai_task_callback.bots.bot_id", &bot.bot_id)));
        let mut seen = Vec::new();
        for (key, id) in bot_ids {
            let Ok(parsed) = uuid::Uuid::parse_str(id.trim()) else {
                return Err(format!(
                    "{key} = \"{id}\" is not a bot id; use the bot's user id in Sylvode (a UUID, the \
                     `ai_participant_id` of its tasks)"
                ));
            };
            if seen.contains(&parsed) {
                return Err(format!(
                    "bot {parsed} has more than one AI task callback token (ai_task_callback.bot_id and \
                     [[ai_task_callback.bots]]); keep one"
                ));
            }
            seen.push(parsed);
        }
        if !cfg.enabled {
            return Ok(());
        }
        let api_url = cfg.api_url.as_deref().unwrap_or_default();
        validate_api_url(api_url)?;
        if !(1..=MAX_AI_TASK_ATTEMPTS).contains(&cfg.max_attempts) {
            return Err(format!(
                "ai_task_callback.max_attempts = {} is out of range; use 1 to {MAX_AI_TASK_ATTEMPTS}",
                cfg.max_attempts
            ));
        }
        if cfg.retry_delay_ms > MAX_AI_TASK_RETRY_DELAY_MS {
            return Err(format!(
                "ai_task_callback.retry_delay_ms = {} is out of range; use at most {MAX_AI_TASK_RETRY_DELAY_MS}",
                cfg.retry_delay_ms
            ));
        }
        for bot in &cfg.bots {
            if let Some(secret) = bot.webhook_secret.as_deref()
                && (secret.trim().is_empty() || is_placeholder(secret))
            {
                return Err(format!(
                    "ai_task_callback.bots (bot {}): webhook_secret is blank or still an example placeholder",
                    bot.bot_id.trim()
                ));
            }
            if !usable_token(&bot.token) {
                return Err(format!(
                    "ai_task_callback is enabled but [[ai_task_callback.bots]] entry for bot {} has no usable \
                     token: set it to that bot's opr_ token",
                    bot.bot_id.trim()
                ));
            }
        }
        if cfg.bot_id.is_some() && !cfg.token.as_deref().is_some_and(usable_token) {
            return Err(
                "ai_task_callback.bot_id is set but ai_task_callback.token is not: set the token of that bot, \
                 or remove bot_id"
                    .to_string(),
            );
        }
        if cfg.bots.is_empty() && !cfg.token.as_deref().is_some_and(usable_token) {
            return Err(
                "ai_task_callback is enabled but has no usable token: set ai_task_callback.token or add a \
                 [[ai_task_callback.bots]] entry with the opr_ token of the bot that is the task's AI \
                 participant"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// The token and transport for bot `task_bot`: the `[[ai_task_callback.bots]]` entry of the
    /// bot, else the global `token` (only for its `bot_id` when that is set). Without one the
    /// answer is [`AiTaskCallback::NoToken`].
    pub fn ai_task_callback_for<'a>(&'a self, task_bot: &str) -> AiTaskCallback<'a> {
        let cfg = &self.ai_task_callback;
        if self.safe_mode || !cfg.enabled {
            return AiTaskCallback::Off;
        }
        let Some(api_url) = cfg.api_url.as_deref() else {
            return AiTaskCallback::Off;
        };
        let target = |token: &'a str, surface: Option<&'a str>, workspace_id: Option<&'a str>| {
            AiTaskCallback::Report(AiTaskCallbackTarget {
                api_url,
                token,
                surface: surface.unwrap_or(cfg.surface.as_str()),
                workspace_id,
            })
        };
        if let Some(bot) = cfg
            .bots
            .iter()
            .find(|bot| same_bot(&bot.bot_id, task_bot) && usable_token(&bot.token))
        {
            return target(&bot.token, bot.surface.as_deref(), bot.workspace_id.as_deref());
        }
        let owner_matches = cfg.bot_id.as_deref().is_none_or(|owner| same_bot(owner, task_bot));
        match cfg.token.as_deref().filter(|token| usable_token(token)) {
            Some(token) if owner_matches => target(token, None, None),
            _ => AiTaskCallback::NoToken,
        }
    }

    /// Whether `ai_task_callback.api_url` is `http://` to a host other than loopback.
    #[must_use]
    pub fn api_url_is_remote_plain_http(&self) -> bool {
        self.ai_task_callback
            .api_url
            .as_deref()
            .and_then(|api_url| url::Url::parse(api_url.trim()).ok())
            .is_some_and(|parsed| parsed.scheme() == "http" && !is_loopback_url(&parsed))
    }

    /// Whether the operator acknowledged a plain-http server address outside loopback and the
    /// configuration uses it with a `runner` agent.
    #[must_use]
    pub fn insecure_transport_acknowledged(&self) -> bool {
        self.ai_task_callback.enabled
            && self.ai_task_callback.allow_insecure_transport
            && self.agents.iter().any(|agent| agent.agent_type == "runner")
            && self.api_url_is_remote_plain_http()
    }

    /// Configurations that start but deserve a prominent warning in the log.
    pub fn startup_warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = self
            .secret_problems()
            .into_iter()
            .filter(|(in_use, _)| !*in_use)
            .map(|(_, problem)| format!("{problem} (not in use now, so start-up continues)"))
            .collect();
        if self.security.allow_unsigned {
            warnings.push(format!(
                "security.allow_unsigned = true: webhook signatures are NOT verified; anyone who can reach \
                 {} can trigger agents. Use this only for local development",
                self.server.listen
            ));
            if self.ai_task_callback.enabled {
                warnings.push(
                    "ai_task_callback is enabled while signatures are not verified: anyone who can reach the \
                     port can make this service complete or fail AI tasks with its bot tokens"
                        .to_string(),
                );
            }
        } else if self.security.webhook_secrets.is_empty() {
            warnings.push(
                "security.webhook_secrets is empty and allow_unsigned = false: every /webhook request will be \
                 answered 401. Add the secret configured on the Sylvode webhook"
                    .to_string(),
            );
        }
        let callback = &self.ai_task_callback;
        if self.insecure_transport_acknowledged() {
            warnings.push(
                "ai_task_callback.allow_insecure_transport = true: the server address is http:// to a host \
                 other than loopback, so bot tokens and every task cross the network in clear text unless \
                 the network itself is encrypted"
                    .to_string(),
            );
        }
        if callback.enabled && callback.bot_id.is_none() && callback.token.as_deref().is_some_and(usable_token) {
            warnings.push(
                "ai_task_callback.token works only for the tasks of the bot that owns it, and it is used for \
                 every task no agent token or [[ai_task_callback.bots]] entry covers: the Sylvode API refuses \
                 it (403) for any other bot's task. Set ai_task_callback.bot_id to the id of the bot that owns \
                 it, and add a [[ai_task_callback.bots]] entry for every other bot this receiver serves"
                    .to_string(),
            );
        }
        warnings
    }

    pub fn tunnel_enabled(&self) -> bool {
        !self.safe_mode && self.features.tunnel_enabled && self.tunnel.as_ref().is_some_and(|t| t.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn safe_mode_turns_every_gated_path_off() {
        let toml = r#"
[server]
listen = "0.0.0.0:9090"

[security]
allow_unsigned = true

[features]
tunnel_enabled = true

[tunnel]
enabled = true
"#;
        let mut cfg: Config = toml::from_str(toml).expect("should parse config");
        assert!(!cfg.safe_mode, "safe mode is never read from the file");
        assert!(cfg.tunnel_enabled());

        cfg.safe_mode = true;
        assert!(!cfg.tunnel_enabled());
    }

    /// Writes `content` to a fresh file and loads it the way start-up does.
    fn load_str(content: &str) -> Result<Config, String> {
        let dir = std::env::temp_dir().join(format!("sylvode-runner-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, content).expect("write config");
        let loaded = Config::load(path.to_str().expect("utf-8 path"));
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
        loaded
    }

    fn route_config(trigger_kinds: &str) -> String {
        format!(
            "[server]\nlisten = \"127.0.0.1:9090\"\n\n[security]\nallow_unsigned = true\n\n\
             [[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"webhook\"\n\n\
             [agents.route]\ntrigger_kinds = {trigger_kinds}\n\n\
             [agents.webhook]\nurl = \"http://127.0.0.1:9000/hook\"\n"
        )
    }

    /// Every value a Sylvode deployment sends is accepted, in any case and with surrounding
    /// whitespace, as the route matcher compares them.
    #[test]
    fn accepts_every_trigger_kind_sylvode_sends() {
        let cfg = load_str(&route_config(
            r#"["assigned", "created", "status_changed", "mentioned", "mention", "proposal_vote", " Manual "]"#,
        ))
        .expect("all wire values are accepted");
        assert_eq!(cfg.agents.len(), 1);
    }

    /// `assignment` was never sent by any Sylvode version, so a route naming it could never match.
    /// Start-up is refused with the valid values instead of silently never dispatching.
    #[test]
    fn refuses_the_removed_completed_and_failed_trigger_kinds() {
        for kind in ["completed", "failed"] {
            let err = load_str(&route_config(&format!(r#"["{kind}"]"#))).expect_err(kind);
            assert!(err.contains(&format!("`{kind}`")), "{err}");
        }
    }

    #[test]
    fn refuses_a_trigger_kind_sylvode_never_sends() {
        let err = load_str(&route_config(r#"["mentioned", "assignment"]"#)).expect_err("assignment is refused");
        assert!(err.contains("`fixer`"), "{err}");
        assert!(err.contains("`assignment`"), "{err}");
        assert!(err.contains("assigned, created"), "{err}");
    }

    fn command_config(agent_type: &str, section: &str) -> String {
        format!(
            "[server]\nlisten = \"127.0.0.1:9090\"\n\n[security]\nallow_unsigned = true\n\n\
             [[agents]]\nid = \"david\"\nname = \"David\"\nagent_type = \"{agent_type}\"\n\n{section}\n"
        )
    }

    /// A command is one program name; a value with whitespace is refused naming the key, whether
    /// or not `args` is also set.
    #[test]
    fn refuses_a_command_with_whitespace() {
        for (agent_type, section, key, args_key) in [
            (
                "openclaw",
                "[agents.openclaw]\ncommand = \"openclaw message send\"\nchannel = \"c\"\ntarget = \"t\"",
                "openclaw.command",
                "openclaw.args",
            ),
            (
                "openclaw",
                "[agents.openclaw]\ncommand = \"openclaw message\"\nargs = [\"send\"]\nchannel = \"c\"\ntarget = \"t\"",
                "openclaw.command",
                "openclaw.args",
            ),
            (
                "openprx",
                "[agents.openprx]\ncommand = \"openprx  message\\tsend\"\ntarget = \"t\"",
                "openprx.command",
                "openprx.args",
            ),
            (
                "custom",
                "[agents.custom]\ncommand = \"echo hello\"",
                "custom.command",
                "custom.args",
            ),
        ] {
            let err = load_str(&command_config(agent_type, section)).expect_err("refused");
            assert_eq!(
                err,
                format!("agent `david`: {key} must name one program; put its arguments in {args_key}"),
                "{agent_type}"
            );
        }
    }

    /// `command` plus `args` is the one form for `openclaw`, `openprx` and `custom`.
    #[test]
    fn accepts_command_with_args() {
        let openclaw = command_config(
            "openclaw",
            "[agents.openclaw]\ncommand = \"openclaw\"\nargs = [\"message\", \"send\"]\nchannel = \"signal\"\ntarget = \"t\"",
        );
        let cfg = load_str(&openclaw).expect("accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.openclaw.as_ref())
            .expect("openclaw agent");
        assert_eq!(agent.command, "openclaw");
        assert_eq!(agent.args, vec!["message", "send"]);

        let openprx = command_config(
            "openprx",
            "[agents.openprx]\ncommand = \"openprx\"\nargs = [\"message\", \"send\"]\ntarget = \"t\"",
        );
        let cfg = load_str(&openprx).expect("accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.openprx.as_ref())
            .expect("openprx agent");
        assert_eq!(agent.command.as_deref(), Some("openprx"));
        assert_eq!(agent.args, vec!["message", "send"]);

        let custom = command_config(
            "custom",
            "[agents.custom]\ncommand = \"echo\"\nargs = [\"hello\", \"{message}\"]",
        );
        let cfg = load_str(&custom).expect("accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.custom.as_ref())
            .expect("custom agent");
        assert_eq!(agent.command, "echo");
        assert_eq!(
            agent.args.as_deref(),
            Some(&["hello".to_string(), "{message}".to_string()][..])
        );
    }

    #[test]
    fn parses_agent_route_config() {
        let toml = r#"
[server]
listen = "0.0.0.0:9090"

[security]
allow_unsigned = true

[[agents]]
id = "contract-review"
name = "Contract Review"
agent_type = "webhook"

[agents.route]
bot_names = ["Document review connection"]
bot_agent_types = ["webhook"]
project_types = ["contract_review"]
trigger_kinds = ["mention"]
events = ["form.record.created"]
form_keys = ["order"]

[agents.webhook]
url = "http://127.0.0.1:9000/hook"
"#;

        let cfg: Config = toml::from_str(toml).expect("should parse config");
        let route = cfg
            .agents
            .first()
            .and_then(|agent| agent.route.as_ref())
            .expect("route config");

        assert_eq!(route.bot_names, vec!["Document review connection"]);
        assert_eq!(route.bot_agent_types, vec!["webhook"]);
        assert_eq!(route.project_types, vec!["contract_review"]);
        assert_eq!(route.trigger_kinds, vec!["mention"]);
        assert_eq!(route.events, vec!["form.record.created"]);
        assert_eq!(route.form_keys, vec!["order"]);
    }

    fn tunnel_config(tunnel_enabled: bool, enabled: bool, extra: &str) -> String {
        format!(
            "[server]\nlisten = \"127.0.0.1:9090\"\n\n[security]\nallow_unsigned = true\n\n\
             [features]\ntunnel_enabled = {tunnel_enabled}\n\n\
             [tunnel]\nenabled = {enabled}\nurl = \"wss://example.invalid/tunnel\"\nagent_id = \"a\"\n\
             auth_token = \"t\"\n{extra}\n"
        )
    }

    /// Requiring an inbound `sig` that nothing verifies authenticates nothing, so start-up is
    /// refused, naming the setting, whether the secret is absent, empty or blank. An empty or
    /// blank secret is refused on its own (see below), and the error names it.
    #[test]
    fn refuses_require_inbound_sig_without_hmac_secret() {
        let err = load_str(&tunnel_config(true, true, "require_inbound_sig = true"))
            .expect_err("require_inbound_sig without a secret is refused");
        assert!(err.contains("tunnel.require_inbound_sig"), "{err}");
        assert!(err.contains("tunnel.hmac_secret"), "{err}");
        for secret in ["hmac_secret = \"\"", "hmac_secret = \"  \""] {
            let err = load_str(&tunnel_config(
                true,
                true,
                &format!("require_inbound_sig = true\n{secret}"),
            ))
            .expect_err("require_inbound_sig with a blank secret is refused");
            assert!(err.contains("tunnel.hmac_secret"), "{err}");
        }
    }

    #[test]
    fn accepts_require_inbound_sig_with_hmac_secret() {
        let cfg = load_str(&tunnel_config(
            true,
            true,
            "require_inbound_sig = true\nhmac_secret = \"shared\"",
        ))
        .expect("require_inbound_sig with a secret is accepted");
        let tunnel = cfg.tunnel.expect("tunnel config");
        assert!(tunnel.require_inbound_sig);
        assert_eq!(tunnel.hmac_secret.as_deref(), Some("shared"));
    }

    fn secrets_config(secrets: &str) -> String {
        format!(
            "[server]\nlisten = \"127.0.0.1:9090\"\n\n[security]\nwebhook_secrets = {secrets}\nallow_unsigned = false\n"
        )
    }

    /// An empty key is a key anyone can sign with, so a blank entry is refused wherever it is in
    /// the list, naming the setting and its position.
    #[test]
    fn refuses_a_blank_webhook_secret() {
        for (secrets, index) in [
            (r#"[""]"#, 0),
            (r#"["   "]"#, 0),
            (r#"["\t"]"#, 0),
            (r#"["a-real-secret", ""]"#, 1),
        ] {
            let err = load_str(&secrets_config(secrets)).expect_err("a blank secret is refused");
            assert!(
                err.contains(&format!("security.webhook_secrets[{index}]")),
                "{secrets}: {err}"
            );
            assert!(err.contains("blank"), "{secrets}: {err}");
        }
        load_str(&secrets_config(r#"["a-real-secret"]"#)).expect("a real secret is accepted");
    }

    /// A blank `hmac_secret` is refused, whatever `require_inbound_sig` says, when the tunnel is in
    /// use: it would sign and verify with an empty key. With the tunnel off it is only a warning.
    #[test]
    fn refuses_a_blank_hmac_secret_when_the_tunnel_is_in_use() {
        for (feature, enabled) in [(true, true), (false, true), (true, false), (false, false)] {
            for secret in ["hmac_secret = \"\"", "hmac_secret = \"   \""] {
                let config = tunnel_config(feature, enabled, &format!("require_inbound_sig = false\n{secret}"));
                if feature && enabled {
                    let err = load_str(&config).expect_err("a blank hmac_secret is refused");
                    assert!(err.contains("tunnel.hmac_secret"), "{err}");
                    assert!(err.contains("blank"), "{err}");
                } else {
                    let cfg = load_str(&config).expect("an unused blank hmac_secret only warns");
                    let warnings = cfg.startup_warnings();
                    assert!(
                        warnings
                            .iter()
                            .any(|w| w.contains("tunnel.hmac_secret") && w.contains("blank")),
                        "{warnings:?}"
                    );
                }
            }
        }
    }

    /// Placeholder checks apply only to secrets in use: the `hmac_secret = "shared-hmac-secret"`
    /// that the earlier standalone examples shipped uncommented, beside `enabled = false`, starts with a
    /// warning; the same line with the tunnel on refuses. A placeholder in `webhook_secrets` is not
    /// used with `allow_unsigned = true` either.
    #[test]
    fn placeholders_refuse_only_secrets_in_use() {
        let old_example = "[server]\nlisten = \"127.0.0.1:9090\"\n\n[security]\nwebhook_secrets = [\"a-real-secret\"]\n\n\
                           [tunnel]\nenabled = false\nurl = \"wss://control.invalid/tunnel\"\nagent_id = \"a\"\n\
                           auth_token = \"t\"\nhmac_secret = \"shared-hmac-secret\"\n";
        let cfg = load_str(old_example).expect("an unused placeholder hmac_secret only warns");
        let warnings = cfg.startup_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("tunnel.hmac_secret") && w.contains("placeholder")),
            "{warnings:?}"
        );
        let err = load_str(
            &old_example
                .replace("enabled = false", "enabled = true")
                .replace("[tunnel]", "[features]\ntunnel_enabled = true\n\n[tunnel]"),
        )
        .expect_err("a placeholder hmac_secret of an enabled tunnel is refused");
        assert!(err.contains("tunnel.hmac_secret"), "{err}");

        let unsigned = load_str(&security_config(
            "127.0.0.1:9090",
            "allow_unsigned = true\nwebhook_secrets = [\"your-secret-here\", \"\"]",
        ))
        .expect("unused webhook secrets only warn");
        let warnings = unsigned.startup_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("security.webhook_secrets[0]") && w.contains("placeholder")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("security.webhook_secrets[1]") && w.contains("blank")),
            "{warnings:?}"
        );
    }

    fn security_config(listen: &str, security: &str) -> String {
        format!("[server]\nlisten = \"{listen}\"\n\n[security]\n{security}\n")
    }

    /// The placeholder of `config.example.toml`, and the ones earlier examples shipped, are
    /// refused: a deployment that copied the example unchanged must not start.
    #[test]
    fn refuses_a_placeholder_secret() {
        let padded: Vec<String> = super::PLACEHOLDER_SECRETS
            .iter()
            .flat_map(|p| [format!("  {p}"), format!("{p}\t"), p.to_ascii_uppercase()])
            .collect();
        let refused = super::PLACEHOLDER_SECRETS
            .iter()
            .copied()
            .chain(padded.iter().map(String::as_str))
            .chain([
                // The compose example of the Sylvode repository.
                "replace_with_webhook_secret",
                // The patterns: replace with, your, changeme, example.
                "REPLACE_WITH_A_REAL_SECRET",
                "Replace-With-Something",
                "your_webhook_secret",
                "YOUR-TOKEN",
                "changeme",
                "ChangeMe-2026",
                "an-example-secret",
                "EXAMPLE",
            ]);
        for placeholder in refused {
            let err = load_str(&security_config(
                "127.0.0.1:9090",
                &format!("webhook_secrets = [\"{placeholder}\"]"),
            ))
            .expect_err("a placeholder webhook secret is refused");
            assert!(err.contains("security.webhook_secrets[0]"), "{err}");
            assert!(err.contains("placeholder"), "{err}");

            let err = load_str(&tunnel_config(true, true, &format!("hmac_secret = \"{placeholder}\"")))
                .expect_err("a placeholder hmac secret is refused");
            assert!(err.contains("tunnel.hmac_secret"), "{err}");
        }
        for chosen in [
            "e2e-webhook-secret",
            "9f86d081884c7d659a2feaa0c55ad015",
            "yours-truly-7Q",
            "replacement",
        ] {
            load_str(&security_config(
                "127.0.0.1:9090",
                &format!("webhook_secrets = [\"{chosen}\"]"),
            ))
            .expect("a chosen secret is accepted");
        }
    }

    /// The example file ships with the placeholder, so it refuses to start as shipped, and starts
    /// once a real secret is filled in.
    #[test]
    fn the_shipped_example_refuses_until_its_secret_is_set() {
        let example = include_str!("../config.example.toml");
        let err = load_str(example).expect_err("the example refuses as shipped");
        assert!(err.contains("placeholder"), "{err}");
        let filled = example.replace(super::EXAMPLE_SECRET_PLACEHOLDER, "a-real-random-secret");
        let cfg = load_str(&filled).expect("the example starts with a real secret");
        assert!(!cfg.security.allow_unsigned, "the example verifies signatures");
        assert_eq!(cfg.server.listen, "127.0.0.1:9090", "the example binds loopback only");
        assert!(
            cfg.agents.is_empty(),
            "the example runs nothing until an agent is added"
        );
        assert!(!cfg.features.tunnel_enabled);
        assert!(!cfg.ai_task_callback.enabled, "AI task callbacks are off by default");
    }

    /// Both example files document `[ai_task_callback]` with its defaults and a placeholder token:
    /// enabling it as shipped is refused, and it starts once a real token is filled in.
    #[test]
    fn the_examples_document_the_ai_task_callback() {
        let examples = [
            (
                include_str!("../config.example.toml"),
                super::EXAMPLE_SECRET_PLACEHOLDER,
            ),
            (
                include_str!("../../../config/sylvode-runner.example.toml"),
                "replace_with_webhook_secret",
            ),
        ];
        for (example, secret) in examples {
            let filled = example.replace(secret, "a-real-random-secret");
            let cfg = load_str(&filled).expect("the example starts with a real secret");
            let callback = &cfg.ai_task_callback;
            assert!(!callback.enabled);
            assert_eq!(callback.surface, "rest");
            assert_eq!(callback.max_attempts, 3);
            assert_eq!(callback.retry_delay_ms, 1000);

            let enabled = filled.replace(
                "[ai_task_callback]\nenabled = false",
                "[ai_task_callback]\nenabled = true",
            );
            assert_ne!(enabled, filled, "the example has an [ai_task_callback] section");
            let err = load_str(&enabled).expect_err("a placeholder token is refused");
            assert!(err.contains("no usable token"), "{err}");

            let token = enabled.replace("replace-with-the-bot-token-of-the-ai-participant", "a-real-bot-token");
            let cfg = load_str(&token).expect("the example starts with a real token");
            assert!(cfg.ai_task_callback.enabled);
        }
    }

    /// `[[ai_task_callback.bots]]` and `ai_task_callback.bot_id`: shape checks, and that the start-up
    /// log says plainly that the global token works only for the bot that owns it.
    #[test]
    fn per_bot_ai_task_tokens_are_validated() {
        const BOT_A: &str = "0b7e2d4c-2f0a-4b8e-8f6c-3a1d9e5c7b20";
        const BOT_B: &str = "5989a9a2-c7f1-4a74-ba05-cffe01e33ad4";
        let base = |callback: &str, agent_extra: &str| {
            format!(
                "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"a-real-random-secret\"]\n\n\
                 [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:8080\"\n{callback}\n\
                 [[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"custom\"\n\
                 [agents.custom]\ncommand = \"/bin/echo\"\n{agent_extra}"
            )
        };
        let bot =
            |id: &str, token: &str| format!("[[ai_task_callback.bots]]\nbot_id = \"{id}\"\ntoken = \"{token}\"\n");

        // Bot tokens alone are enough for an agent without a token of its own.
        let cfg = load_str(&base(&bot(BOT_B, "opr_b"), "")).expect("bot tokens alone start");
        assert_eq!(cfg.ai_task_callback.bots.len(), 1);

        let err = load_str(&base(&bot("Fixer", "opr_b"), "")).expect_err("a name is no bot id");
        assert!(err.contains("is not a bot id"), "{err}");

        let twice = format!("{}{}", bot(BOT_B, "opr_b"), bot(&BOT_B.to_uppercase(), "opr_b2"));
        let err = load_str(&base(&twice, "")).expect_err("one token per bot");
        assert!(err.contains("more than one"), "{err}");
        let err = load_str(&base(
            &format!("token = \"opr_a\"\nbot_id = \"{BOT_B}\"\n{}", bot(BOT_B, "opr_b")),
            "",
        ))
        .expect_err("the global owner and an entry name the same bot");
        assert!(err.contains("more than one"), "{err}");

        let err = load_str(&base(&bot(BOT_B, "replace-with-the-bot-token"), "")).expect_err("placeholder");
        assert!(err.contains("no usable"), "{err}");

        let err = load_str(&base(&format!("bot_id = \"{BOT_A}\"\n{}", bot(BOT_B, "opr_b")), ""))
            .expect_err("an owner without a token");
        assert!(err.contains("bot_id is set but ai_task_callback.token is not"), "{err}");

        // A token per agent no longer exists: the bots' tokens are the only ones.
        let err = load_str(&base(
            &bot(BOT_B, "opr_b"),
            "[agents.ai_task_callback]\ntoken = \"opr_agent\"\n",
        ))
        .expect_err("no agent-level token");
        assert!(err.contains("unknown field `ai_task_callback`"), "{err}");

        // Without bot_id the global token is used for every bot, and the log says it works only
        // for its owner.
        let legacy = load_str(&base("token = \"opr_a\"\n", "")).expect("legacy config starts");
        assert!(
            legacy
                .startup_warnings()
                .iter()
                .any(|w| w.contains("works only for the tasks of the bot that owns it")),
            "{:?}",
            legacy.startup_warnings()
        );
        let owned = load_str(&base(&format!("token = \"opr_a\"\nbot_id = \"{BOT_A}\"\n"), "")).expect("owned token");
        assert!(
            !owned
                .startup_warnings()
                .iter()
                .any(|w| w.contains("works only for the tasks of the bot that owns it"))
        );
    }

    /// Unsigned requests on an interface other than loopback are refused unless acknowledged.
    #[test]
    fn refuses_allow_unsigned_on_a_non_loopback_address_without_acknowledgement() {
        for listen in [
            "0.0.0.0:9090",
            "[::]:9090",
            "192.168.1.5:9090",
            "example.com:9090",
            // Names and addresses that only look like loopback.
            "localhost.evil:9090",
            "localhost.:9090",
            "127.0.0.1.evil.com:9090",
            "[::ffff:127.0.0.1]:9090",
            "[2001:db8::1]:9090",
            "[fe80::1]:9090",
            "[evil.com]:9090",
            "user@localhost:9090",
            "localhost:9090@evil.com:1",
        ] {
            let err = load_str(&security_config(listen, "allow_unsigned = true"))
                .expect_err("unsigned on a non-loopback address is refused");
            assert!(err.contains("security.allow_unsigned"), "{listen}: {err}");
            assert!(err.contains("security.allow_unsigned_non_loopback"), "{listen}: {err}");

            let acknowledged = load_str(&security_config(
                listen,
                "allow_unsigned = true\nallow_unsigned_non_loopback = true",
            ))
            .expect("acknowledged");
            assert!(acknowledged.security.allow_unsigned_non_loopback);
        }
        for listen in [
            "127.0.0.1:9090",
            "127.0.0.2:0",
            "[::1]:9090",
            "localhost:9090",
            "LOCALHOST:1",
        ] {
            load_str(&security_config(listen, "allow_unsigned = true")).expect("loopback is accepted");
        }
    }

    #[test]
    fn startup_warnings_name_unsigned_and_unverifiable_configurations() {
        let unsigned = load_str(&security_config("127.0.0.1:9090", "allow_unsigned = true")).expect("loads");
        let warnings = unsigned.startup_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("allow_unsigned = true") && w.contains("NOT verified")),
            "{warnings:?}"
        );

        let public = load_str(&security_config(
            "0.0.0.0:9090",
            "allow_unsigned = true\nallow_unsigned_non_loopback = true",
        ))
        .expect("loads");
        assert!(
            public.startup_warnings().iter().any(|w| w.contains("0.0.0.0:9090")),
            "{:?}",
            public.startup_warnings()
        );

        let no_secret = load_str(&security_config("127.0.0.1:9090", "allow_unsigned = false")).expect("loads");
        assert!(
            no_secret
                .startup_warnings()
                .iter()
                .any(|w| w.contains("webhook_secrets") && w.contains("401")),
            "{:?}",
            no_secret.startup_warnings()
        );

        let fine = load_str(&security_config("0.0.0.0:9090", "webhook_secrets = [\"real\"]")).expect("loads");
        assert!(fine.startup_warnings().is_empty(), "{:?}", fine.startup_warnings());
    }

    /// `ws://` sends the bearer token in clear text, so it is refused for a host other than
    /// loopback unless acknowledged.
    #[test]
    fn refuses_plain_ws_to_a_non_loopback_host_without_acknowledgement() {
        let with_url =
            |url: &str, extra: &str| tunnel_config(true, true, extra).replace("wss://example.invalid/tunnel", url);
        for url in [
            "ws://control.example.com/tunnel",
            "ws://10.0.0.5:8080/t",
            "WS://example.com",
            // Hosts that only look like loopback, and loopback behind userinfo.
            "ws://localhost.evil/t",
            "ws://localhost:x@evil.com/t",
            "ws://127.0.0.1:x@evil.com/t",
            "ws://user:pw@127.0.0.1/t",
            "ws://user@localhost/t",
            "ws://127.0.0.1.evil.com/t",
            "ws://[2001:db8::1]/t",
            "ws://[::ffff:127.0.0.1]/t",
            "ws://[evil.com]/t",
        ] {
            let err = load_str(&with_url(url, "")).expect_err("plain ws to a remote host is refused");
            assert!(err.contains("tunnel.url"), "{url}: {err}");
            assert!(err.contains("tunnel.allow_insecure_transport"), "{url}: {err}");
            load_str(&with_url(url, "allow_insecure_transport = true")).expect("acknowledged");
        }
        for url in [
            "ws://127.0.0.1:8080/t",
            "ws://localhost/t",
            "ws://[::1]:9/t",
            "wss://control.example.com/t",
        ] {
            load_str(&with_url(url, "")).expect("loopback or TLS is accepted");
        }
        load_str(&tunnel_config(true, false, "").replace("wss://example.invalid/tunnel", "ws://example.com"))
            .expect("a disabled tunnel is not checked");
    }

    /// Without `require_inbound_sig`, or with the tunnel off, a missing secret is still accepted.
    #[test]
    fn accepts_a_missing_hmac_secret_when_nothing_requires_it() {
        load_str(&tunnel_config(true, true, "require_inbound_sig = false")).expect("no requirement");
        load_str(&tunnel_config(true, true, "")).expect("requirement defaults to off");
        load_str(&tunnel_config(false, true, "require_inbound_sig = true")).expect("feature flag off");
        load_str(&tunnel_config(true, false, "require_inbound_sig = true")).expect("tunnel disabled");
    }

    /// The configuration block Sylvode hands out when a bot is connected to a project, byte for
    /// byte as the API's contract test fixture holds it.
    const GOLDEN_BLOCK: &str = include_str!("../../api/tests/fixtures/runner-config-block.toml");

    const GOLDEN_BOT: &str = "00000000-0000-0000-0000-000000000000";

    /// A `runner` agent sends the bot tokens to `api_url`: `http://` to a host other than loopback
    /// is refused unless the operator acknowledges it, and the refusal names the key.
    #[test]
    fn plain_http_to_a_remote_server_needs_the_acknowledgement() {
        let with_url = |url: &str, extra: &str| {
            GOLDEN_BLOCK.replace(
                "api_url = \"https://sylvode.example.net\"",
                &format!("api_url = \"{url}\"\n{extra}"),
            )
        };
        for url in [
            "http://192.0.2.10:8080",
            "http://sylvode.example.net",
            "http://[2001:db8::1]:8080",
        ] {
            let err = Config::from_toml(&with_url(url, "")).expect_err("plain http to a remote host is refused");
            assert!(
                err.contains("ai_task_callback.allow_insecure_transport"),
                "{url}: {err}"
            );
            let cfg = Config::from_toml(&with_url(url, "allow_insecure_transport = true")).expect("acknowledged");
            assert!(cfg.insecure_transport_acknowledged(), "{url}");
            let warnings = cfg.startup_warnings();
            assert_eq!(
                warnings
                    .iter()
                    .filter(|w| w.contains("ai_task_callback.allow_insecure_transport"))
                    .count(),
                1,
                "one warning line: {warnings:?}"
            );
        }
        for url in [
            "https://sylvode.example.net",
            "http://127.0.0.1:8080",
            "http://localhost:8080",
        ] {
            let cfg = Config::from_toml(&with_url(url, "")).expect("https and loopback http are accepted");
            assert!(!cfg.insecure_transport_acknowledged(), "{url}");
            // The key set on a safe address is not a warning.
            let cfg = Config::from_toml(&with_url(url, "allow_insecure_transport = true")).expect("accepted");
            assert!(!cfg.insecure_transport_acknowledged(), "{url}");
            assert!(
                !cfg.startup_warnings()
                    .iter()
                    .any(|w| w.contains("allow_insecure_transport")),
                "{url}"
            );
        }
        assert!(
            !Config::from_toml(&with_url("https://sylvode.example.net", ""))
                .expect("loads")
                .ai_task_callback
                .allow_insecure_transport,
            "the default is false"
        );
    }

    /// Two blocks Sylvode issued for two bots, merged the way the block's header says: the shared
    /// sections once, the second bot's `[[ai_task_callback.bots]]` and `[[agents]]` tables appended.
    /// The runner then serves two bots, and each bot's own secret verifies only its own deliveries.
    #[test]
    fn two_issued_blocks_merged_load() {
        const SECOND_BOT: &str = "00000000-0000-0000-0000-000000000002";
        const SECOND_SECRET: &str = "second-fixture-signing-secret";
        let second = GOLDEN_BLOCK
            .replace(GOLDEN_BOT, SECOND_BOT)
            .replace("golden-fixture-signing-secret", SECOND_SECRET)
            .replace("opr_golden_fixture_token", "opr_second_fixture_token")
            .replace("Build bot", "Second bot");
        let tables_from = |text: &str, header: &str| -> String {
            let start = text.find(&format!("\n{header}\n")).expect("the table is in the block") + 1;
            let rest = &text[start..];
            let end = rest[header.len()..]
                .find("\n[")
                .map_or(rest.len(), |offset| header.len() + offset);
            rest[..end].trim_end().to_string()
        };
        // With more than one runner agent each needs its own, disjoint `allowed_roots` (the block's
        // header says so; the global one is for a runner with a single agent).
        let with_roots = |text: &str, root: &str| {
            text.replace(
                "agent_type = \"runner\"\n",
                &format!("agent_type = \"runner\"\nallowed_roots = [\"{root}\"]\n"),
            )
        };
        let second = with_roots(&second, "/srv/dev/work/second");
        let mut merged = with_roots(GOLDEN_BLOCK, "/srv/dev/work/svwl").trim_end().to_string();
        merged.push_str("\n\n");
        merged.push_str(&tables_from(&second, "[[ai_task_callback.bots]]"));
        merged.push_str("\n\n");
        // The agent table and its route table.
        let agent_start = second.find("\n[[agents]]\n").expect("agent table") + 1;
        merged.push_str(second[agent_start..].trim_end());
        merged.push('\n');

        let cfg = Config::from_toml(&merged).expect("the merged file is accepted");
        assert_eq!(cfg.agents.len(), 2, "both bots' agents");
        assert_eq!(cfg.ai_task_callback.bots.len(), 2, "both bots' credentials");
        assert_eq!(
            cfg.webhook_secrets_for(Some(SECOND_BOT)),
            vec![SECOND_SECRET.to_string()]
        );
        assert_eq!(
            cfg.webhook_secrets_for(Some(GOLDEN_BOT)),
            vec!["golden-fixture-signing-secret".to_string()],
            "the first bot keeps its own secret"
        );
        // The shared sections stay as the first block had them.
        assert_eq!(cfg.runner.allowed_roots, vec!["/srv/dev/work/svwl".to_string()]);
        assert_eq!(cfg.server.listen, "127.0.0.1:9090");
    }

    #[test]
    fn the_golden_configuration_block_loads_as_one_runner_agent() {
        let cfg = Config::from_toml(GOLDEN_BLOCK).expect("the block Sylvode issues is accepted");
        assert_eq!(cfg.agents.len(), 1);
        let agent = cfg.agents.first().expect("agent");
        assert_eq!(agent.agent_type, "runner");
        assert_eq!(agent.id, GOLDEN_BOT);
        assert_eq!(agent.name, "Build bot");
        assert_eq!(
            agent.route.as_ref().map(|r| r.bot_ids.clone()),
            Some(vec![GOLDEN_BOT.to_string()])
        );
        assert_eq!(cfg.runner.allowed_roots, vec!["/srv/dev/work/svwl".to_string()]);
        assert!(cfg.ai_task_callback.enabled);
        let bot = cfg.ai_task_callback.bots.first().expect("bot");
        assert_eq!(
            (bot.bot_id.as_str(), bot.surface.as_deref()),
            (GOLDEN_BOT, Some("mcp_stdio"))
        );
        // Every other runner setting has its default.
        assert_eq!(cfg.runner.delivery_check_timeout_secs, 5);
        assert_eq!(cfg.runner.mcp.tool_mode, "search");
        assert!(!cfg.runner.cli.codex.enabled && !cfg.runner.cli.opencode.enabled);
        assert_eq!(cfg.runner.tmux.socket_name, "");
        assert_eq!(
            cfg.runner.cli.claude.launch_command,
            ["claude", "--permission-mode", "bypassPermissions"]
        );
        assert_eq!(
            cfg.runner.cli.claude.headless_args,
            ["--permission-mode", "bypassPermissions"]
        );
        assert!(cfg.runner.cli.claude.hooks && !cfg.runner.mcp.allow_write_call);
        assert!(cfg.runner.cli.codex.headless_args.is_empty());
        let codex = &cfg.runner.cli.codex;
        assert_eq!(
            codex.launch_command,
            [
                "codex",
                "--no-daemon",
                "--no-alt-screen",
                "--dangerously-bypass-approvals-and-sandbox"
            ]
        );
        assert!(codex.hooks && codex.process_names == ["codex"] && codex.push_settle_ms == 700);
        // The agent resolves a token for its bot, with the surface the block registered.
        match cfg.ai_task_callback_for(GOLDEN_BOT) {
            super::AiTaskCallback::Report(target) => {
                assert_eq!(target.surface, "mcp_stdio");
                assert_eq!(target.api_url, "https://sylvode.example.net");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A key a newer Sylvode might add is refused naming it, in every section the block uses.
    #[test]
    fn an_unknown_key_in_the_block_is_refused_naming_it() {
        for (anchor, extra, key) in [
            ("[server]\n", "mystery_key = 1\n", "mystery_key"),
            ("[security]\n", "mystery_key = 1\n", "mystery_key"),
            ("[ai_task_callback]\n", "mystery_key = 1\n", "mystery_key"),
            ("[runner]\n", "mystery_key = 1\n", "mystery_key"),
            ("[[ai_task_callback.bots]]\n", "mystery_key = 1\n", "mystery_key"),
            ("[[agents]]\n", "mystery_key = 1\n", "mystery_key"),
            ("[agents.route]\n", "mystery_key = 1\n", "mystery_key"),
        ] {
            let block = GOLDEN_BLOCK.replacen(anchor, &format!("{anchor}{extra}"), 1);
            assert_ne!(block, GOLDEN_BLOCK, "{anchor}");
            let error = Config::from_toml(&block).expect_err(anchor);
            assert!(error.contains(key), "{anchor}: {error}");
        }
        let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n[runner.tmux]\nmystery_key = 1\n")).expect_err("tmux");
        assert!(error.contains("mystery_key"), "{error}");
        let error =
            Config::from_toml(&format!("{GOLDEN_BLOCK}\n[runner.cli.claude]\nenabled = true\n")).expect_err("claude");
        assert!(error.contains("enabled"), "claude has no enabled key: {error}");
        let error = Config::from_toml(
            "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n[nonsense]\nx = 1\n",
        )
        .expect_err("top");
        assert!(error.contains("nonsense"), "{error}");
    }

    #[test]
    fn a_runner_agent_needs_the_callback_and_an_mcp_stdio_token() {
        let swap = |from: &str, to: &str| GOLDEN_BLOCK.replacen(from, to, 1);
        let error =
            Config::from_toml(&swap("enabled = true\napi_url", "enabled = false\napi_url")).expect_err("callback off");
        assert!(
            error.contains("ai_task_callback") && error.contains("enabled"),
            "{error}"
        );
        // A configuration without roots loads: no session may then be created and no headless run
        // started, but a bot's existing sessions are still served.
        Config::from_toml(&swap("allowed_roots = [\"/srv/dev/work/svwl\"]", "allowed_roots = []")).expect("no roots");
        for surface in ["rest", "mcp_http", "cli"] {
            let error = Config::from_toml(&swap("surface = \"mcp_stdio\"", &format!("surface = \"{surface}\"")))
                .expect_err(surface);
            assert_eq!(
                error,
                format!("bot {GOLDEN_BOT}: runner bots need a token registered for mcp_stdio")
            );
        }
        // The bot's workspace is what the MCP server the runner starts needs: a missing, nil or
        // malformed one is refused at load time, not when the first task starts.
        let line = "workspace_id = \"11111111-1111-4111-8111-111111111111\"\n";
        for replacement in [
            "",
            "workspace_id = \"00000000-0000-0000-0000-000000000000\"\n",
            "workspace_id = \"x\"\n",
        ] {
            let error = Config::from_toml(&swap(line, replacement)).expect_err(replacement);
            assert!(error.contains(GOLDEN_BOT) && error.contains("workspace_id"), "{error}");
        }
        let cfg = Config::from_toml(GOLDEN_BLOCK).expect("golden");
        let super::AiTaskCallback::Report(target) = cfg.ai_task_callback_for(GOLDEN_BOT) else {
            panic!("the golden bot has a token");
        };
        assert_eq!(target.workspace_id, Some("11111111-1111-4111-8111-111111111111"));
        // A bot of the route without any token.
        let other = "11111111-1111-4111-8111-111111111111";
        let error = Config::from_toml(&swap(
            &format!("bot_ids = [\"{GOLDEN_BOT}\"]"),
            &format!("bot_ids = [\"{GOLDEN_BOT}\", \"{other}\"]"),
        ))
        .expect_err("second bot");
        assert!(error.contains(other) && error.contains("no token"), "{error}");
        // The route must name bots by UUID.
        let error = Config::from_toml(
            &swap(&format!("bot_ids = [\"{GOLDEN_BOT}\"]"), "bot_names = [\"Build bot\"]").replacen(
                &format!("[[agents]]\nid = \"{GOLDEN_BOT}\""),
                "[[agents]]\nid = \"build\"",
                1,
            ),
        )
        .expect_err("by name");
        assert!(error.contains("route.bot_ids"), "{error}");
        // No agent type table next to a runner agent.
        let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n[agents.custom]\ncommand = \"echo\"\n"))
            .expect_err("custom table");
        assert!(error.contains("[runner.cli.*]"), "{error}");
    }

    #[test]
    fn runner_settings_are_validated() {
        for (extra, key) in [
            ("[runner.tmux]\nmin_version = \"3.1\"\n", "min_version"),
            ("[runner.tmux]\nmin_version = \"three\"\n", "min_version"),
            ("[runner.tmux]\nsocket_name = \"a b\"\n", "socket_name"),
            (
                &format!("[runner.tmux]\nsocket_name = \"{}\"\n", "a".repeat(33)),
                "socket_name",
            ),
            ("[runner.tmux]\nwindow_width = 5\n", "window_width"),
            ("[runner.mcp]\ntool_mode = \"both\"\n", "tool_mode"),
            ("[runner.cli.claude]\nbinary = \"\"\n", "claude.binary"),
            ("[runner.cli.claude]\nlaunch_command = []\n", "launch_command"),
            (
                "[runner.cli.claude]\nlaunch_command = [\"--verbose\"]\n",
                "launch_command",
            ),
            (
                "[runner.cli.claude]\nlaunch_command = [\"claude\", \"a\\nb\"]\n",
                "launch_command",
            ),
            ("[runner.cli.claude]\nheadless_args = [\"a\\u0007\"]\n", "headless_args"),
            (
                "[runner.cli.codex]\nheadless_args = [\"a\\nb\"]\n",
                "codex.headless_args",
            ),
            ("[runner.cli.claude]\nprocess_names = []\n", "process_names"),
        ] {
            // These sections go after `[runner]` in the block; a table header may appear once, so
            // the block's own `[runner]` stays and the sub-table is appended.
            let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n{extra}")).expect_err(key);
            assert!(error.contains(key), "{key}: {error}");
        }
        let ok = format!(
            "{GOLDEN_BLOCK}\n[runner.tmux]\nmin_version = \"3.2\"\nsocket_name = \"\"\n[runner.mcp]\ntool_mode = \"full\"\n"
        );
        assert!(Config::from_toml(&ok).is_ok());
    }

    /// Each start-up refusal of a `[[runner.tmux.sessions]]` entry names its place in the array.
    #[test]
    fn a_bad_session_entry_is_refused_naming_its_index() {
        let entry = |project: &str, cli: &str, name: &str| {
            format!("[[runner.tmux.sessions]]\nproject = \"{project}\"\ncli = \"{cli}\"\nname = \"{name}\"\n")
        };
        let good = entry("SV", "claude", "sv");
        for (extra, want) in [
            (entry("sv", "claude", "x"), "runner.tmux.sessions[0].project"),
            (entry("", "claude", "x"), "runner.tmux.sessions[0].project"),
            (entry("SV", "opencode", "x"), "runner.tmux.sessions[0].cli"),
            (entry("SV", "bash", "x"), "runner.tmux.sessions[0].cli"),
            (entry("SV", "claude", "a:b"), "runner.tmux.sessions[0].name"),
            (entry("SV", "claude", "-x"), "runner.tmux.sessions[0].name"),
            (entry("SV", "claude", "a.b"), "runner.tmux.sessions[0].name"),
            (entry("SV", "claude", "SV-claude"), "runner.tmux.sessions[0].name"),
            (entry("SV", "claude", "rf-Codex"), "runner.tmux.sessions[0].name"),
            (entry("SV", "claude", &"a".repeat(65)), "runner.tmux.sessions[0].name"),
            (
                format!("{good}{}", entry("SV", "claude", "other")),
                "runner.tmux.sessions[1] repeats",
            ),
            (
                format!("{good}{}", entry("RF", "codex", "SV")),
                "runner.tmux.sessions[1].name repeats",
            ),
            (
                format!("{good}{}{}", entry("RF", "claude", "rf"), entry("PRC", "claude", "Rf")),
                "runner.tmux.sessions[2].name repeats",
            ),
        ] {
            let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n{extra}")).expect_err(want);
            assert!(error.contains(want), "{want}: {error}");
        }
        // A key the entry does not know is refused naming it.
        let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n{good}create = true\n")).expect_err("unknown key");
        assert!(error.contains("create"), "{error}");
        // The same project may name a session per CLI, and the names may look like plain words.
        let two = format!("{good}{}", entry("SV", "codex", "sv-codex-lead"));
        let cfg = Config::from_toml(&format!("{GOLDEN_BLOCK}\n{two}")).expect("two entries");
        assert_eq!(cfg.runner.tmux.sessions.len(), 2);
        assert_eq!(
            cfg.runner.session_override("SV", "codex").map(|e| e.name.as_str()),
            Some("sv-codex-lead")
        );
    }

    /// A configuration with no `[[runner.tmux.sessions]]` entry has none, and so every session is
    /// derived as before.
    #[test]
    fn without_session_entries_nothing_is_configured() {
        let cfg = Config::from_toml(GOLDEN_BLOCK).expect("loads");
        assert!(cfg.runner.tmux.sessions.is_empty());
        assert!(cfg.runner.session_override("SV", "claude").is_none());
    }

    /// A deployment without a `runner` agent is unaffected by the `[runner]` block.
    #[test]
    fn without_a_runner_agent_the_runner_block_asks_for_nothing() {
        let cfg = Config::from_toml("[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n")
            .expect("loads");
        assert!(cfg.runner.allowed_roots.is_empty());
    }

    /// Bot tokens never appear in a debug rendering of the configuration.
    #[test]
    fn debug_output_of_the_configuration_hides_every_bot_token() {
        let cfg = Config::from_toml(GOLDEN_BLOCK).expect("loads");
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("opr_"), "{shown}");
        assert!(shown.contains("<redacted>"));
        let with_global = Config::from_toml(
            "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
             [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:2\"\ntoken = \"opr_zzzzzzzzzzzz\"\n\
             [[agents]]\nid = \"a\"\nname = \"a\"\nagent_type = \"custom\"\n[agents.custom]\ncommand = \"echo\"\n",
        )
        .expect("loads");
        let shown = format!("{with_global:?}");
        assert!(!shown.contains("opr_zzz"), "{shown}");
    }

    #[test]
    fn a_runner_agent_takes_no_agent_level_token_and_a_delivery_needs_its_own_bot() {
        let error = Config::from_toml(&format!(
            "{GOLDEN_BLOCK}\n[agents.ai_task_callback]\ntoken = \"opr_{}\"\nsurface = \"rest\"\n",
            "q1".repeat(14)
        ))
        .expect_err("agent-level token");
        assert!(error.contains("unknown field `ai_task_callback`"), "{error}");
    }

    #[test]
    fn the_api_url_must_be_https_unless_the_host_is_loopback() {
        let with = |url: &str| GOLDEN_BLOCK.replace("https://sylvode.example.net", url);
        for ok in [
            "https://sylvode.example.net",
            "http://127.0.0.1:8083",
            "http://localhost:8083",
            "http://[::1]:8083",
        ] {
            assert!(Config::from_toml(&with(ok)).is_ok(), "{ok}");
        }
        for bad in [
            "http://sylvode.example.net",
            "http://10.1.2.3:8083",
            "http://localhost.evil.net",
            "http://192.0.2.7",
        ] {
            let error = Config::from_toml(&with(bad)).expect_err(bad);
            assert!(
                error.contains("http://") && error.contains("loopback"),
                "{bad}: {error}"
            );
        }
    }

    /// No secret of any kind appears in a debug rendering of the configuration.
    #[test]
    fn debug_output_of_the_configuration_holds_no_secret_of_any_kind() {
        let marker = |name: &str| format!("SECRET-{name}-{}", "z".repeat(12));
        let toml = format!(
            "[server]\nlisten = \"127.0.0.1:1\"\n\
             [security]\nwebhook_secrets = [\"{w1}\", \"{w2}\"]\n\
             [features]\ntunnel_enabled = true\n\
             [tunnel]\nenabled = true\nurl = \"wss://127.0.0.1:9\"\nauth_token = \"{tunnel}\"\nhmac_secret = \"{hmac}\"\n\
             [ai_task_callback]\nenabled = true\napi_url = \"https://api.test.net\"\ntoken = \"opr_{global}\"\nbot_id = \"{BOT}\"\n\
             [[ai_task_callback.bots]]\nbot_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{bot}\"\nwebhook_secret = \"{bot_secret}\"\n\
             [[agents]]\nid = \"hook\"\nname = \"hook\"\nagent_type = \"webhook\"\n[agents.webhook]\nurl = \"https://h.test.net\"\nsecret = \"{hook}\"\n\
             [[agents]]\nid = \"fwd\"\nname = \"fwd\"\nagent_type = \"custom\"\n[agents.custom]\ncommand = \"echo\"\n",
            BOT = "00000000-0000-4000-8000-000000000009",
            w1 = marker("w1"),
            w2 = marker("w2"),
            tunnel = marker("tunnel"),
            hmac = marker("hmac"),
            global = marker("global"),
            bot = marker("bot"),
            bot_secret = marker("botsecret"),
            hook = marker("hook"),
        );
        let cfg = Config::from_toml(&toml).expect("loads");
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("SECRET-"), "{shown}");
        assert!(!shown.contains("zzzzzzzz"), "{shown}");
    }

    /// A delivery is verified only with the secret of the bot it names.
    #[test]
    fn a_webhook_secret_is_bound_to_its_bot() {
        let a = "00000000-0000-4000-8000-0000000000aa";
        let b = "00000000-0000-4000-8000-0000000000bb";
        let bot = |id: &str, secret: Option<&str>| {
            format!(
                "[[ai_task_callback.bots]]\nbot_id = \"{id}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{}\"\nsurface = \"mcp_stdio\"\n{}\n",
                "w3".repeat(14),
                secret.map_or(String::new(), |s| format!("webhook_secret = \"{s}\""))
            )
        };
        let agents = format!(
            "[[agents]]\nid = \"{a}\"\nname = \"a\"\nagent_type = \"runner\"\nallowed_roots = [\"/x/a\"]\n[agents.route]\nbot_ids = [\"{a}\"]\n\
             [[agents]]\nid = \"{b}\"\nname = \"b\"\nagent_type = \"runner\"\nallowed_roots = [\"/x/b\"]\n[agents.route]\nbot_ids = [\"{b}\"]\n"
        );
        let head = "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nwebhook_secrets = [\"shared-sig-1\"]\n\
                    [ai_task_callback]\nenabled = true\napi_url = \"https://api.test.net\"\n\
                    [runner]\nallowed_roots = [\"/x\"]\n";
        let load = |bots: String| Config::from_toml(&format!("{head}{bots}{agents}"));
        // Two bots without their own secrets are refused: one bot's secret could sign for the other.
        let error = load(format!("{}{}", bot(a, None), bot(b, None))).expect_err("ambiguous");
        assert!(error.contains("webhook_secret"), "{error}");
        let cfg = load(format!(
            "{}{}",
            bot(a, Some("secret-for-a-1")),
            bot(b, Some("secret-for-b-1"))
        ))
        .expect("loads");
        assert_eq!(cfg.webhook_secrets_for(Some(a)), vec!["secret-for-a-1".to_string()]);
        assert_eq!(
            cfg.webhook_secrets_for(Some(&a.to_uppercase())),
            vec!["secret-for-a-1".to_string()]
        );
        assert_eq!(cfg.webhook_secrets_for(Some(b)), vec!["secret-for-b-1".to_string()]);
        // A delivery for another or no bot falls back to the shared list, which holds neither.
        let shared = vec!["shared-sig-1".to_string()];
        assert_eq!(
            cfg.webhook_secrets_for(Some("00000000-0000-4000-8000-0000000000cc")),
            shared
        );
        assert_eq!(cfg.webhook_secrets_for(None), shared);
        // A placeholder or blank bot secret is refused.
        assert!(load(format!("{}{}", bot(a, Some("")), bot(b, Some("secret-for-b-1")))).is_err());
        assert!(
            load(format!(
                "{}{}",
                bot(a, Some("replace-with-it")),
                bot(b, Some("secret-for-b-1"))
            ))
            .is_err()
        );
    }

    /// Every table refuses a key it does not know.
    #[test]
    fn every_table_refuses_an_unknown_key() {
        for (table, extra) in [
            ("[runner.cli.codex]", "enabled = false\nmystery = 1\n"),
            ("[runner.cli.opencode]", "mystery = 1\n"),
            ("[runner.mcp]", "mystery = 1\n"),
            ("[runtime]", "mystery = 1\n"),
            ("[features]", "mystery = 1\n"),
            ("[tunnel]", "mystery = 1\n"),
        ] {
            let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n{table}\n{extra}")).expect_err(table);
            assert!(error.contains("mystery"), "{table}: {error}");
        }
        let error = Config::from_toml(
            "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
             [[agents]]\nid = \"c\"\nname = \"c\"\nagent_type = \"custom\"\n[agents.custom]\ncommand = \"echo\"\nmystery = 1\n",
        )
        .expect_err("agents.custom");
        assert!(error.contains("mystery"), "{error}");
    }

    /// An agent type the runner does not know is refused with the list of valid ones (`cli` was one
    /// that is gone); the settings that only it used are unknown keys.
    #[test]
    fn an_unknown_agent_type_and_the_retired_cli_settings_are_refused() {
        let base = |extra: &str| {
            format!(
                "[server]\nlisten = \"127.0.0.1:9090\"\n[security]\nwebhook_secrets = [\"a-real-secret-value\"]\n{extra}"
            )
        };
        let err = load_str(&base(
            "[[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"cli\"\n",
        ))
        .expect_err("a cli agent is refused");
        assert!(
            err.contains("agent `fixer`")
                && err.contains("unknown agent_type `cli`")
                && err.contains("valid values: runner, openclaw, openprx, webhook, custom"),
            "{err}"
        );
        for (extra, key) in [
            ("[features]\ncli_enabled = true\n", "cli_enabled"),
            ("[features]\ncallback_enabled = true\n", "callback_enabled"),
            ("[ai_task_callback]\nmax_output_chars = 10\n", "max_output_chars"),
            (
                "[[agents]]\nid = \"a\"\nname = \"A\"\nagent_type = \"webhook\"\n[agents.cli]\nexecutor = \"codex\"\n",
                "cli",
            ),
        ] {
            let err = load_str(&base(extra)).expect_err(extra);
            assert!(err.contains("unknown field") && err.contains(key), "{extra}: {err}");
        }
    }

    /// Every key the operator-owned session model removed is refused naming the key.
    #[test]
    fn removed_keys_are_refused_by_name() {
        for (table, key) in [
            ("", "trust_project_files = []"),
            ("[runner.tmux]", "enabled = true"),
            ("[runner.cli.claude]", "permission_mode = \"dontAsk\""),
            ("[runner.cli.claude]", "allow_write_call = true"),
            ("[runner.cli.claude]", "allowed_tools = []"),
            ("[runner.cli.claude]", "disallowed_tools = []"),
            ("[runner.cli.claude]", "dangerously_bypass_permissions = true"),
            ("[runner.cli.claude]", "env_passthrough = [\"CLAUDE_CONFIG_DIR\"]"),
            ("[runner.cli.codex]", "sandbox = \"read-only\""),
            ("[runner.cli.codex]", "approval = \"never\""),
            ("[runner.cli.codex]", "dangerously_bypass_permissions = true"),
            ("[runner.cli.codex]", "env_passthrough = []"),
            ("[runner.cli.opencode]", "process_names = [\"opencode\"]"),
            ("[runner.cli.opencode]", "env_passthrough = []"),
            ("[runner.mcp]", "mcp_config_path = \"/x\""),
        ] {
            let name = key.split(' ').next().unwrap_or(key);
            let text = if table.is_empty() {
                // A key of `[runner]` itself goes right after its header.
                GOLDEN_BLOCK.replacen("[runner]\n", &format!("[runner]\n{key}\n"), 1)
            } else {
                format!("{GOLDEN_BLOCK}\n{table}\n{key}\n")
            };
            let error = Config::from_toml(&text).expect_err(key);
            assert!(
                error.contains("unknown field") && error.contains(name),
                "{key}: {error}"
            );
        }
    }

    /// The new keys load and `allow_write_call` lives under `[runner.mcp]`.
    #[test]
    fn the_session_keys_load() {
        let config = Config::from_toml(&format!(
            "{GOLDEN_BLOCK}\n[runner.cli.claude]\nlaunch_command = [\"claude\", \"--model\", \"x\"]\n\
             headless_args = []\nhooks = false\n[runner.cli.codex]\nheadless_args = [\"--sandbox\", \"read-only\"]\n\
             [runner.mcp]\nallow_write_call = true\n"
        ))
        .expect("loads");
        assert_eq!(config.runner.cli.claude.launch_command, ["claude", "--model", "x"]);
        assert!(!config.runner.cli.claude.hooks && config.runner.mcp.allow_write_call);
        assert_eq!(config.runner.cli.codex.headless_args, ["--sandbox", "read-only"]);
    }

    /// The codex session keys load, validate per key, and the settle time is the longer of the
    /// global and the codex value.
    #[test]
    fn the_codex_session_keys_load_and_validate() {
        let config = Config::from_toml(&format!(
            "{GOLDEN_BLOCK}\n[runner.cli.codex]\nlaunch_command = [\"codex\", \"--no-alt-screen\", \"{{notify_override}}\"]\n\
             hooks = false\nprocess_names = [\"codex\", \"node\"]\npush_settle_ms = 900\n"
        ))
        .expect("loads");
        let settings = config.runner.cli_settings("codex").expect("codex settings");
        assert_eq!(settings.launch_command.len(), 3);
        assert!(!config.runner.cli.codex.hooks);
        assert_eq!(settings.process_names, ["codex", "node"]);
        assert!(config.runner.cli_settings("opencode").is_none());
        assert_eq!(config.runner.push_settle_ms("codex"), 900);
        assert_eq!(config.runner.push_settle_ms("claude"), 300);
        // The global value wins when it is longer.
        let longer =
            Config::from_toml(&format!("{GOLDEN_BLOCK}\n[runner.tmux]\npush_settle_ms = 1500\n")).expect("loads");
        assert_eq!(longer.runner.push_settle_ms("codex"), 1500);
        assert_eq!(longer.runner.push_settle_ms("claude"), 1500);
        for (key, line) in [
            ("codex.launch_command", "launch_command = []"),
            ("codex.launch_command", "launch_command = [\"--no-daemon\"]"),
            ("codex.launch_command", "launch_command = [\"codex\", \"a\\nb\"]"),
            ("codex.process_names", "process_names = []"),
            ("codex.process_names", "process_names = [\" \"]"),
            ("codex.push_settle_ms", "push_settle_ms = 499"),
            ("codex.push_settle_ms", "push_settle_ms = 10001"),
        ] {
            let error = Config::from_toml(&format!("{GOLDEN_BLOCK}\n[runner.cli.codex]\n{line}\n")).expect_err(line);
            assert!(error.contains(&format!("runner.cli.{key}")), "{line}: {error}");
        }
        let at_floor = Config::from_toml(&format!("{GOLDEN_BLOCK}\n[runner.cli.codex]\npush_settle_ms = 500\n"));
        assert!(at_floor.is_ok());
    }

    #[test]
    fn a_second_runner_agent_without_its_own_roots_is_refused() {
        let other = "00000000-0000-0000-0000-000000000002";
        let error = Config::from_toml(&format!(
            "{GOLDEN_BLOCK}\n[[ai_task_callback.bots]]\nbot_id = \"{other}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_{t}\"\nsurface = \"mcp_stdio\"\n\
             [[agents]]\nid = \"{other}\"\nname = \"B\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{other}\"]\n",
            t = "z8".repeat(14)
        ))
        .expect_err("refused");
        assert!(
            error.contains("allowed_roots") || error.contains("webhook_secret"),
            "{error}"
        );
    }
}
