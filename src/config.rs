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
    /// Safe mode, resolved once at start-up from `SYLVODE_WEBHOOK_SAFE_MODE` (or the legacy
    /// `OPENPR_WEBHOOK_SAFE_MODE`). Not read from the file.
    #[serde(skip)]
    pub safe_mode: bool,
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

/// Refuses a command that contains whitespace but names no program.
///
/// Commands run without a shell, so `openclaw message send` is looked up as one program of that
/// name, which does not exist. A path or program name that really contains whitespace is accepted.
fn validate_command(agent_id: &str, key: &str, command: &str) -> Result<(), String> {
    if !command.contains(char::is_whitespace) || names_a_program(command) {
        return Ok(());
    }
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let args: Vec<String> = words.map(|word| format!("{word:?}")).collect();
    Err(format!(
        "agent `{agent_id}`: {key} `{command}` contains whitespace and is not an executable; commands run \
         without a shell, so write the program and its arguments separately: command = {program:?} and \
         args = [{}]",
        args.join(", ")
    ))
}

/// Whether `command` names an existing file, either as a path or as a name found on `PATH`, the
/// way the command is resolved when it is run.
fn names_a_program(command: &str) -> bool {
    let path = std::path::Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(command).is_file()))
}

impl Config {
    pub fn load(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("failed to read config {path}: {e}"))?;
        let config: Self = toml::from_str(&content).map_err(|e| format!("failed to parse config: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// Refuses settings that parse but can never work, so they fail at start-up instead of
    /// silently never dispatching.
    fn validate(&self) -> Result<(), String> {
        self.validate_secrets()?;
        self.validate_tunnel()?;
        for agent in &self.agents {
            let commands = [
                ("openclaw.command", agent.openclaw.as_ref().map(|c| c.command.as_str())),
                (
                    "openprx.command",
                    agent.openprx.as_ref().and_then(|c| c.command.as_deref()),
                ),
                ("custom.command", agent.custom.as_ref().map(|c| c.command.as_str())),
            ];
            for (key, command) in commands {
                if let Some(command) = command {
                    validate_command(&agent.id, key, command)?;
                }
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

    /// Refuses a blank inbound secret. An HMAC with an empty key is one anyone can compute, so a
    /// blank entry (an unfilled template, say) would turn signature checking into no check.
    fn validate_secrets(&self) -> Result<(), String> {
        if let Some(index) = self
            .security
            .webhook_secrets
            .iter()
            .position(|secret| secret.trim().is_empty())
        {
            return Err(format!(
                "security.webhook_secrets[{index}] is blank; an empty HMAC key lets anyone sign a request. \
                 Set it to the secret configured on the Sylvode webhook, or remove the entry"
            ));
        }
        let blank_hmac = self
            .tunnel
            .as_ref()
            .and_then(|t| t.hmac_secret.as_deref())
            .is_some_and(|secret| secret.trim().is_empty());
        if blank_hmac {
            return Err(
                "tunnel.hmac_secret is blank; an empty HMAC key lets anyone sign an envelope. Set it \
                        to the secret shared with the control plane, or remove the setting"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Refuses `require_inbound_sig` on an enabled tunnel without `hmac_secret`.
    ///
    /// Without a secret the `sig` of an inbound envelope is never verified, so requiring one would
    /// only check that the field exists and would authenticate nothing.
    fn validate_tunnel(&self) -> Result<(), String> {
        let Some(tunnel) = self
            .tunnel
            .as_ref()
            .filter(|t| self.features.tunnel_enabled && t.enabled)
        else {
            return Ok(());
        };
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
            "[server]\nlisten = \"0.0.0.0:9090\"\n\n[security]\nallow_unsigned = true\n\n\
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
            "[server]\nlisten = \"0.0.0.0:9090\"\n\n[security]\nallow_unsigned = true\n\n\
             [[agents]]\nid = \"david\"\nname = \"David\"\nagent_type = \"{agent_type}\"\n\n{section}\n"
        )
    }

    /// Commands run without a shell, so `openclaw message send` names a program that does not
    /// exist. Start-up is refused with the `command` + `args` form to use instead.
    #[test]
    fn refuses_a_multi_word_command() {
        let openclaw = command_config(
            "openclaw",
            "[agents.openclaw]\ncommand = \"openclaw message send\"\nchannel = \"signal\"\ntarget = \"t\"",
        );
        let err = load_str(&openclaw).expect_err("multi-word openclaw command is refused");
        assert!(err.contains("agent `david`"), "{err}");
        assert!(err.contains("openclaw.command"), "{err}");
        assert!(err.contains(r#"command = "openclaw""#), "{err}");
        assert!(err.contains(r#"args = ["message", "send"]"#), "{err}");

        let openprx = command_config(
            "openprx",
            "[agents.openprx]\ncommand = \"openprx message send\"\ntarget = \"t\"",
        );
        assert!(load_str(&openprx).expect_err("refused").contains("openprx.command"));

        let custom = command_config("custom", "[agents.custom]\ncommand = \"echo hello\"");
        assert!(load_str(&custom).expect_err("refused").contains("custom.command"));
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
            "[server]\nlisten = \"0.0.0.0:9090\"\n\n[security]\nallow_unsigned = true\n\n\
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

    /// A present but blank `hmac_secret` is refused whatever `require_inbound_sig` says and
    /// whether or not the tunnel is enabled: it would sign and verify with an empty key.
    #[test]
    fn refuses_a_blank_hmac_secret_whenever_it_is_present() {
        for (feature, enabled) in [(true, true), (false, true), (true, false), (false, false)] {
            for secret in ["hmac_secret = \"\"", "hmac_secret = \"   \""] {
                let err = load_str(&tunnel_config(
                    feature,
                    enabled,
                    &format!("require_inbound_sig = false\n{secret}"),
                ))
                .expect_err("a blank hmac_secret is refused");
                assert!(err.contains("tunnel.hmac_secret"), "{err}");
                assert!(err.contains("blank"), "{err}");
            }
        }
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
