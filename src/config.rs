use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
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
    pub openclaw: Option<OpenClawConfig>,
    pub openprx: Option<OpenPRXConfig>,
    pub webhook: Option<WebhookAgentConfig>,
    pub custom: Option<CustomConfig>,
    pub cli: Option<CliAgentConfig>,
    pub message_template: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct OpenClawConfig {
    pub command: String,
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
    /// Signal daemon HTTP API base URL (e.g. http://127.0.0.1:8686)
    pub signal_api: Option<String>,
    /// Target recipient (phone number or uuid)
    pub target: String,
    /// Account phone number for signal-cli
    pub account: Option<String>,
    /// Or use CLI command (e.g. "openprx message send")
    pub command: Option<String>,
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
}

fn default_timeout_secs() -> u64 {
    900
}

fn default_max_output_chars() -> usize {
    12000
}

fn default_reconnect_secs() -> u64 {
    3
}

fn default_heartbeat_secs() -> u64 {
    20
}

impl Config {
    pub fn load(path: &str) -> Self {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read config {}: {}", path, e));
        toml::from_str(&content).unwrap_or_else(|e| panic!("Failed to parse config: {}", e))
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
        let agent = &cfg.agents[0];
        let cli = agent.cli.as_ref().expect("cli config should exist");

        assert_eq!(agent.agent_type, "cli");
        assert_eq!(cli.executor, "codex");
        assert_eq!(cli.timeout_secs, 900);
        assert_eq!(cli.max_output_chars, 12000);
        assert_eq!(cli.callback.as_deref(), Some("mcp"));
    }
}
