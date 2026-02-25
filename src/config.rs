use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub security: SecurityConfig,
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
    pub webhook: Option<WebhookAgentConfig>,
    pub custom: Option<CustomConfig>,
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
pub struct CustomConfig {
    pub command: String,
    pub args: Option<Vec<String>>,
}

impl Config {
    pub fn load(path: &str) -> Self {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read config {}: {}", path, e));
        toml::from_str(&content).unwrap_or_else(|e| panic!("Failed to parse config: {}", e))
    }
}
