use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
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
    /// Safe mode, resolved once at start-up from `SYLVODE_WEBHOOK_SAFE_MODE` (or the legacy
    /// `OPENPR_WEBHOOK_SAFE_MODE`). Not read from the file.
    #[serde(skip)]
    pub safe_mode: bool,
    /// Deprecation notices for commands written in the one-string form, logged at start-up.
    #[serde(skip)]
    pub legacy_command_notices: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct FeatureConfig {
    #[serde(default)]
    pub tunnel_enabled: bool,
    #[serde(default)]
    pub cli_enabled: bool,
    #[serde(default)]
    pub callback_enabled: bool,
}

#[derive(Deserialize, Clone, Debug)]
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
pub struct ServerConfig {
    pub listen: String,
}

#[derive(Deserialize, Clone, Debug)]
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
    pub cli: Option<CliAgentConfig>,
    pub message_template: Option<String>,
    /// Per-agent overrides of `[ai_task_callback]` (`[agents.ai_task_callback]`).
    #[serde(default)]
    pub ai_task_callback: Option<AgentAiTaskCallbackConfig>,
}

/// Reports the outcome of a worker AI task back to the Sylvode API.
///
/// The Sylvode worker marks an AI task `processing` when it delivers it, and only
/// `POST /api/v1/ai/callbacks/task/{task_id}/complete` or `.../fail` ends that state. With this
/// enabled, an agent that ran such a task (a delivery carrying `task_id` and `ai_participant_id`)
/// calls the matching endpoint once the run ends.
#[derive(Deserialize, Clone, Debug)]
pub struct AiTaskCallbackConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Base URL of the Sylvode API, for example `http://127.0.0.1:8080`. Required when enabled.
    pub api_url: Option<String>,
    /// Bearer token: the `opr_` token of the bot that is the task's AI participant (the API only
    /// lets the assigned participant, or a system administrator, end a task). An agent can
    /// override it with `[agents.ai_task_callback].token`.
    pub token: Option<String>,
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
    /// At most this many trailing characters of the agent's output go into the callback.
    #[serde(default = "default_ai_task_max_output_chars")]
    pub max_output_chars: usize,
}

impl Default for AiTaskCallbackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: None,
            token: None,
            surface: default_ai_task_surface(),
            max_attempts: default_ai_task_max_attempts(),
            retry_delay_ms: default_ai_task_retry_delay_ms(),
            max_output_chars: default_ai_task_max_output_chars(),
        }
    }
}

/// Per-agent overrides of [`AiTaskCallbackConfig`].
#[derive(Deserialize, Clone, Debug, Default)]
pub struct AgentAiTaskCallbackConfig {
    /// Whether this agent reports AI task outcomes. Defaults to true for every agent type except
    /// `webhook`, which forwards the delivery to another receiver that owns the task.
    pub enabled: Option<bool>,
    pub token: Option<String>,
    pub surface: Option<String>,
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
}

#[derive(Deserialize, Clone, Debug, Default)]
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
pub struct OpenClawConfig {
    pub command: String,
    /// Fixed arguments placed between `command` and `--channel`.
    #[serde(default)]
    pub args: Vec<String>,
    pub channel: String,
    pub target: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct WebhookAgentConfig {
    pub url: String,
    pub secret: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
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
pub struct CustomConfig {
    pub command: String,
    pub args: Option<Vec<String>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CliAgentConfig {
    pub executor: String,
    pub workdir: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_output_chars")]
    pub max_output_chars: usize,
    pub prompt_template: Option<String>,
    pub callback: Option<String>,
    pub callback_url: Option<String>,
    pub callback_token: Option<String>,
    pub update_state_on_start: Option<String>,
    pub update_state_on_success: Option<String>,
    pub update_state_on_fail: Option<String>,
    /// MCP instructions appended to the CLI prompt, enabling AI agents
    /// to read full issue context and write results back via MCP tools.
    pub mcp_instructions: Option<String>,
    /// When true, the callback will not include a state update field,
    /// because the AI agent updates issue state directly via MCP.
    #[serde(default)]
    pub skip_callback_state: bool,
    /// Path to an MCP config file passed to claude-code via `--mcp-config`.
    pub mcp_config_path: Option<String>,
    /// Extra environment variables injected into the executor subprocess.
    #[serde(default)]
    pub env_vars: std::collections::HashMap<String, String>,
}

#[derive(Deserialize, Clone, Debug)]
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

const fn default_timeout_secs() -> u64 {
    900
}

const fn default_max_output_chars() -> usize {
    12000
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

const fn default_ai_task_max_output_chars() -> usize {
    4000
}

/// Upper bound of `ai_task_callback.max_attempts`.
const MAX_AI_TASK_ATTEMPTS: u32 = 10;
/// Upper bound of `ai_task_callback.retry_delay_ms`.
const MAX_AI_TASK_RETRY_DELAY_MS: u64 = 60_000;

/// The secret `config.example.toml` ships with. Start-up is refused while it is still in place.
pub const EXAMPLE_SECRET_PLACEHOLDER: &str = "replace-with-the-secret-of-your-sylvode-webhook";

/// Secrets copied from an example file rather than chosen: the current example's placeholder, the
/// values earlier examples and the README showed, and the placeholder of the compose example in
/// the Sylvode repository (`config/sylvode-webhook.example.toml`).
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

/// Whether `agent` reports worker AI task outcomes when `[ai_task_callback]` is enabled.
fn agent_reports_ai_tasks(agent: &AgentConfig) -> bool {
    agent
        .ai_task_callback
        .as_ref()
        .and_then(|o| o.enabled)
        .unwrap_or(agent.agent_type != "webhook")
}

/// The token `agent` reports AI task outcomes with: its own, else the global one.
fn agent_ai_task_token<'a>(cfg: &'a AiTaskCallbackConfig, agent: &'a AgentConfig) -> Option<&'a str> {
    agent
        .ai_task_callback
        .as_ref()
        .and_then(|o| o.token.as_deref())
        .or(cfg.token.as_deref())
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

/// Every trigger kind a Sylvode deployment sends, the closed set `route.trigger_kinds` may name.
///
/// A direct webhook delivery carries `bot_context.trigger_reason`: `created`, `assigned`,
/// `status_changed`, `mentioned`, `completed` or `failed`. A worker-dispatched AI task carries a
/// top-level `trigger_kind`: `assigned`, `mention`, `proposal_vote` or `manual`.
pub const TRIGGER_KINDS: [&str; 9] = [
    "assigned",
    "created",
    "status_changed",
    "mentioned",
    "completed",
    "failed",
    "mention",
    "proposal_vote",
    "manual",
];

/// A command written as one string, `program arg...`, as earlier documentation showed it
/// (`command = "openclaw message send"`).
///
/// Commands run without a shell, so such a string is accepted only in the one form whose meaning
/// is unambiguous: the whole string names no file, `args` is empty, the string contains no quote
/// or backslash (so there is no shell quoting to interpret), and its first word names a program.
/// It is then split on whitespace into the program and its arguments, once, at start-up; text
/// substituted later is never split. Any other command containing whitespace that names no file
/// is refused, showing the `command` and `args` to write instead.
///
/// Returns the deprecation notice when the command was split.
fn resolve_command(
    agent_id: &str,
    key: &str,
    command: &mut String,
    args_empty: bool,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<Option<Vec<String>>, String> {
    if !command.contains(char::is_whitespace) || names_a_program(command, path_var) {
        return Ok(None);
    }
    let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    let (program, rest) = words.split_first().map_or(("", &[][..]), |(p, r)| (p.as_str(), r));
    let quoted = rest
        .iter()
        .map(|word| format!("{word:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let obstacle = if command.contains(['"', '\'', '\\']) {
        Some("it contains quotes or backslashes, which only a shell would interpret".to_string())
    } else if !args_empty {
        Some(format!("{} is also set", key.replace("command", "args")))
    } else if !names_a_program(program, path_var) {
        Some(format!("`{program}` is not a file or a program on PATH either"))
    } else {
        None
    };
    if let Some(obstacle) = obstacle {
        return Err(format!(
            "agent `{agent_id}`: {key} `{command}` contains whitespace and is not an executable, and cannot be \
             split unambiguously ({obstacle}); commands run without a shell, so write the program and its \
             arguments separately: command = {program:?} and args = [{quoted}]"
        ));
    }
    let split = rest.to_vec();
    *command = program.to_string();
    Ok(Some(split))
}

/// Whether `command` names an existing file, either as a path or as a name found on `path_var`
/// (the `PATH` the command is resolved with when it is run).
fn names_a_program(command: &str, path_var: Option<&std::ffi::OsStr>) -> bool {
    let path = std::path::Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    !command.is_empty()
        && path_var.is_some_and(|dirs| std::env::split_paths(dirs).any(|dir| dir.join(command).is_file()))
}

fn legacy_command_notice(agent_id: &str, key: &str, original: &str, program: &str, args: &[String]) -> String {
    let quoted = args
        .iter()
        .map(|word| format!("{word:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "agent `{agent_id}`: {key} = {original:?} is the deprecated one-string form; it runs as command = \
         {program:?} with args = [{quoted}]. Write it that way: commands run without a shell"
    )
}

impl Config {
    pub fn load(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("failed to read config {path}: {e}"))?;
        Self::from_toml(&content, std::env::var_os("PATH").as_deref())
    }

    /// Parses and validates a configuration, resolving commands against `path_var`.
    pub fn from_toml(content: &str, path_var: Option<&std::ffi::OsStr>) -> Result<Self, String> {
        let mut config: Self = toml::from_str(content).map_err(|e| format!("failed to parse config: {e}"))?;
        config.resolve_commands(path_var)?;
        config.validate()?;
        Ok(config)
    }

    /// Splits the deprecated one-string command form, or refuses an unusable command; see
    /// [`resolve_command`]. Each split adds a notice to `legacy_command_notices`.
    fn resolve_commands(&mut self, path_var: Option<&std::ffi::OsStr>) -> Result<(), String> {
        let mut notices = Vec::new();
        for agent in &mut self.agents {
            let id = agent.id.as_str();
            if let Some(cfg) = agent.openclaw.as_mut() {
                let original = cfg.command.clone();
                if let Some(split) =
                    resolve_command(id, "openclaw.command", &mut cfg.command, cfg.args.is_empty(), path_var)?
                {
                    notices.push(legacy_command_notice(
                        id,
                        "openclaw.command",
                        &original,
                        &cfg.command,
                        &split,
                    ));
                    cfg.args = split;
                }
            }
            if let Some(cfg) = agent.openprx.as_mut()
                && let Some(command) = cfg.command.as_mut()
            {
                let original = command.clone();
                if let Some(split) = resolve_command(id, "openprx.command", command, cfg.args.is_empty(), path_var)? {
                    notices.push(legacy_command_notice(id, "openprx.command", &original, command, &split));
                    cfg.args = split;
                }
            }
            if let Some(cfg) = agent.custom.as_mut() {
                let original = cfg.command.clone();
                let args_empty = cfg.args.as_ref().is_none_or(Vec::is_empty);
                if let Some(split) = resolve_command(id, "custom.command", &mut cfg.command, args_empty, path_var)? {
                    notices.push(legacy_command_notice(
                        id,
                        "custom.command",
                        &original,
                        &cfg.command,
                        &split,
                    ));
                    cfg.args = Some(split);
                }
            }
        }
        self.legacy_command_notices = notices;
        Ok(())
    }

    /// Refuses settings that parse but can never work, so they fail at start-up instead of
    /// silently never dispatching.
    fn validate(&self) -> Result<(), String> {
        self.validate_secrets()?;
        self.validate_tunnel()?;
        self.validate_ai_task_callback()?;
        for agent in &self.agents {
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
        let surfaces = std::iter::once(("ai_task_callback.surface".to_string(), cfg.surface.as_str())).chain(
            self.agents.iter().filter_map(|agent| {
                let surface = agent.ai_task_callback.as_ref()?.surface.as_deref()?;
                Some((format!("agent `{}`: ai_task_callback.surface", agent.id), surface))
            }),
        );
        for (key, surface) in surfaces {
            if !AI_TASK_SURFACES.contains(&surface) {
                return Err(format!(
                    "{key} = \"{surface}\" is not a transport a Sylvode bot token can be registered with; \
                     valid values: {}",
                    AI_TASK_SURFACES.join(", ")
                ));
            }
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
        for agent in self.agents.iter().filter(|agent| agent_reports_ai_tasks(agent)) {
            let token = agent_ai_task_token(cfg, agent).unwrap_or_default();
            if token.trim().is_empty() || is_placeholder(token) {
                return Err(format!(
                    "ai_task_callback is enabled but agent `{}` has no usable token: set \
                     ai_task_callback.token or [agents.ai_task_callback].token to the opr_ token of the bot \
                     that is the task's AI participant, or set [agents.ai_task_callback].enabled = false",
                    agent.id
                ));
            }
        }
        Ok(())
    }

    /// Where and how `agent` reports the outcome of a worker AI task; `None` when it does not
    /// (the feature is off, safe mode is on, or the agent opts out).
    pub fn ai_task_callback_target<'a>(&'a self, agent: &'a AgentConfig) -> Option<AiTaskCallbackTarget<'a>> {
        let cfg = &self.ai_task_callback;
        if self.safe_mode || !cfg.enabled || !agent_reports_ai_tasks(agent) {
            return None;
        }
        let surface = agent
            .ai_task_callback
            .as_ref()
            .and_then(|o| o.surface.as_deref())
            .unwrap_or(cfg.surface.as_str());
        Some(AiTaskCallbackTarget {
            api_url: cfg.api_url.as_deref()?,
            token: agent_ai_task_token(cfg, agent)?,
            surface,
        })
    }

    /// Where and how to fail a worker AI task that no agent matches: the global API URL, token and
    /// surface. `None` when the feature is off, safe mode is on, or there is no usable global token
    /// (agents may each carry their own, but an unmatched task has no agent).
    pub fn ai_task_callback_unmatched_target(&self) -> Option<AiTaskCallbackTarget<'_>> {
        let cfg = &self.ai_task_callback;
        if self.safe_mode || !cfg.enabled {
            return None;
        }
        let token = cfg
            .token
            .as_deref()
            .filter(|token| !token.trim().is_empty() && !is_placeholder(token))?;
        Some(AiTaskCallbackTarget {
            api_url: cfg.api_url.as_deref()?,
            token,
            surface: cfg.surface.as_str(),
        })
    }

    /// Configurations that start but deserve a prominent warning in the log.
    pub fn startup_warnings(&self) -> Vec<String> {
        let mut warnings = self.legacy_command_notices.clone();
        warnings.extend(
            self.secret_problems()
                .into_iter()
                .filter(|(in_use, _)| !*in_use)
                .map(|(_, problem)| format!("{problem} (not in use now, so start-up continues)")),
        );
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
        warnings
    }

    pub const fn cli_enabled(&self) -> bool {
        !self.safe_mode && self.features.cli_enabled
    }

    pub const fn callback_enabled(&self) -> bool {
        !self.safe_mode && self.features.callback_enabled
    }

    pub fn tunnel_enabled(&self) -> bool {
        !self.safe_mode && self.features.tunnel_enabled && self.tunnel.as_ref().is_some_and(|t| t.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_cli_agent_config() {
        let toml = r#"
[server]
listen = "0.0.0.0:9090"

[security]
webhook_secrets = ["s"]
allow_unsigned = false

[[agents]]
id = "vano-cli"
name = "Vano CLI"
agent_type = "cli"

[agents.cli]
executor = "codex"
workdir = "/tmp"
callback = "mcp"
callback_url = "http://127.0.0.1:8090/mcp/rpc"
"#;

        let cfg: Config = toml::from_str(toml).expect("should parse config");
        let agent = cfg.agents.first().expect("should have agent");
        let cli = agent.cli.as_ref().expect("cli config should exist");

        assert_eq!(agent.agent_type, "cli");
        assert_eq!(cli.executor, "codex");
        assert_eq!(cli.timeout_secs, 900);
        assert_eq!(cli.max_output_chars, 12000);
        assert_eq!(cli.callback.as_deref(), Some("mcp"));
        assert!(!cfg.features.cli_enabled);
        assert_eq!(cfg.runtime.http_timeout_secs, 15);
        assert_eq!(cfg.runtime.cli_max_concurrency, 1);
    }

    #[test]
    fn safe_mode_turns_every_gated_path_off() {
        let toml = r#"
[server]
listen = "0.0.0.0:9090"

[security]
allow_unsigned = true

[features]
tunnel_enabled = true
cli_enabled = true
callback_enabled = true

[tunnel]
enabled = true
"#;
        let mut cfg: Config = toml::from_str(toml).expect("should parse config");
        assert!(!cfg.safe_mode, "safe mode is never read from the file");
        assert!(cfg.cli_enabled() && cfg.callback_enabled() && cfg.tunnel_enabled());

        cfg.safe_mode = true;
        assert!(!cfg.cli_enabled());
        assert!(!cfg.callback_enabled());
        assert!(!cfg.tunnel_enabled());
    }

    /// Writes `content` to a fresh file and loads it the way start-up does.
    fn load_str(content: &str) -> Result<Config, String> {
        let dir = std::env::temp_dir().join(format!("sylvode-webhook-config-{}", uuid::Uuid::new_v4()));
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
            r#"["assigned", "created", "status_changed", "mentioned", "completed", "failed", "mention", "proposal_vote", " Manual "]"#,
        ))
        .expect("all wire values are accepted");
        assert_eq!(cfg.agents.len(), 1);
    }

    /// `assignment` was never sent by any Sylvode version, so a route naming it could never match.
    /// Start-up is refused with the valid values instead of silently never dispatching.
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

    /// A directory holding empty executables named `names`, used as `PATH`.
    struct FakePath(std::path::PathBuf);

    impl FakePath {
        fn with(names: &[&str]) -> Self {
            let dir = std::env::temp_dir().join(format!("sylvode webhook path {}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("create dir");
            for name in names {
                std::fs::write(dir.join(name), "#!/bin/sh\n").expect("write program");
            }
            Self(dir)
        }

        fn var(&self) -> std::ffi::OsString {
            self.0.clone().into_os_string()
        }
    }

    impl Drop for FakePath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn parse_with_path(content: &str, path: &FakePath) -> Result<Config, String> {
        Config::from_toml(content, Some(path.var().as_os_str()))
    }

    /// The one-string form earlier documentation showed, `openclaw message send`, runs as
    /// `openclaw` with `args = ["message", "send"]`, with a deprecation notice naming the
    /// replacement. The same holds for `openprx` and `custom` commands.
    #[test]
    fn splits_the_documented_one_string_command_form() {
        let path = FakePath::with(&["openclaw", "openprx", "echo"]);
        let openclaw = command_config(
            "openclaw",
            "[agents.openclaw]\ncommand = \"openclaw message send\"\nchannel = \"signal\"\ntarget = \"t\"",
        );
        let cfg = parse_with_path(&openclaw, &path).expect("the documented form is accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.openclaw.as_ref())
            .expect("openclaw agent");
        assert_eq!(agent.command, "openclaw");
        assert_eq!(agent.args, vec!["message", "send"]);
        let warnings = cfg.startup_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("agent `david`")
                && w.contains(r#"openclaw.command = "openclaw message send""#)
                && w.contains(r#"command = "openclaw" with args = ["message", "send"]"#)),
            "{warnings:?}"
        );

        let openprx = command_config(
            "openprx",
            "[agents.openprx]\ncommand = \"openprx  message\tsend\"\ntarget = \"t\"",
        );
        let cfg = parse_with_path(&openprx, &path).expect("accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.openprx.as_ref())
            .expect("openprx agent");
        assert_eq!(agent.command.as_deref(), Some("openprx"));
        assert_eq!(agent.args, vec!["message", "send"]);

        let custom = command_config("custom", "[agents.custom]\ncommand = \"echo hello\"");
        let cfg = parse_with_path(&custom, &path).expect("accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.custom.as_ref())
            .expect("custom agent");
        assert_eq!(agent.command, "echo");
        assert_eq!(agent.args.as_deref(), Some(&["hello".to_string()][..]));
    }

    /// Any other multi-word command that names no file is refused with the `command` + `args`
    /// form to write: when `args` is also set, when it contains shell quoting, or when its first
    /// word is not a program either.
    #[test]
    fn refuses_a_multi_word_command_that_cannot_be_split_unambiguously() {
        let path = FakePath::with(&["openclaw", "echo"]);
        for (section, why) in [
            (
                "[agents.openclaw]\ncommand = \"openclaw message send\"\nargs = [\"x\"]\nchannel = \"c\"\ntarget = \"t\"",
                "openclaw.args is also set",
            ),
            (
                "[agents.openclaw]\ncommand = 'openclaw \"message send\"'\nchannel = \"c\"\ntarget = \"t\"",
                "quotes",
            ),
            (
                "[agents.openclaw]\ncommand = \"missing-tool message send\"\nchannel = \"c\"\ntarget = \"t\"",
                "`missing-tool` is not a file or a program on PATH",
            ),
        ] {
            let err = parse_with_path(&command_config("openclaw", section), &path).expect_err("refused");
            assert!(err.contains("agent `david`"), "{err}");
            assert!(err.contains("openclaw.command"), "{err}");
            assert!(err.contains(why), "{why}: {err}");
            assert!(err.contains("args = ["), "{err}");
        }
        let custom = command_config(
            "custom",
            "[agents.custom]\ncommand = \"echo hello\"\nargs = [\"{message}\"]",
        );
        assert!(
            parse_with_path(&custom, &path)
                .expect_err("refused")
                .contains("custom.args is also set")
        );
    }

    /// A program name that really contains whitespace is found on `PATH` and used as it is.
    #[test]
    fn accepts_a_program_name_with_whitespace_found_on_path() {
        let path = FakePath::with(&["my tool"]);
        let custom = command_config("custom", "[agents.custom]\ncommand = \"my tool\"");
        let cfg = parse_with_path(&custom, &path).expect("a program on PATH is accepted");
        let agent = cfg
            .agents
            .first()
            .and_then(|a| a.custom.as_ref())
            .expect("custom agent");
        assert_eq!(agent.command, "my tool");
        assert_eq!(agent.args, None);
        assert!(
            cfg.legacy_command_notices.is_empty(),
            "{:?}",
            cfg.legacy_command_notices
        );
    }

    /// A path with a space in it is fine when it names an existing file.
    #[test]
    fn accepts_an_existing_executable_path_with_whitespace() {
        let dir = std::env::temp_dir().join(format!("sylvode webhook {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create dir");
        let tool = dir.join("my tool");
        std::fs::write(&tool, "#!/bin/sh\n").expect("write tool");
        let custom = command_config(
            "custom",
            &format!("[agents.custom]\ncommand = '{}'", tool.to_str().expect("utf-8 path")),
        );
        let loaded = load_str(&custom);
        std::fs::remove_dir_all(&dir).expect("remove dir");
        loaded.expect("an existing path is accepted");
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
    /// that the examples up to 0.3.17 shipped uncommented, beside `enabled = false`, starts with a
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
            cfg.agents
                .iter()
                .all(|agent| agent.agent_type == "cli" || agent.agent_type == "webhook"),
            "every example agent that runs a command is gated by cli_enabled"
        );
        assert!(!cfg.features.cli_enabled);
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
                include_str!("../../../config/sylvode-webhook.example.toml"),
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
            assert_eq!(callback.max_output_chars, 4000);

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
}
