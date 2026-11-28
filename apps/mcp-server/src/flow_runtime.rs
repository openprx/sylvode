//! Explicit Flow-only process settings. Workspace-bot MCP configuration validation is unchanged.
use crate::client::{ClientConfig, OpenPrClient, transport_label};
use platform::config::{McpTransport, OpenPrConfig, Secret};
use std::path::Path;

#[derive(Debug)]
pub struct FlowRuntime {
    pub api_url: String,
    pub workspace_id: Option<uuid::Uuid>,
    pub transport: McpTransport,
    pub bind_addr: String,
    pub credential: Option<Secret>,
    /// `[mcp] allow_write_call` of the configuration file; `true` without one.
    pub allow_write_call: bool,
}

pub fn parse_access_token(value: &str) -> Result<Secret, std::convert::Infallible> {
    // Parsing cannot fail in clap: clap would echo the supplied credential in its error.
    Ok(Secret::new(value))
}

pub fn access_token(value: &str) -> Result<Secret, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 8192 || value.chars().any(char::is_whitespace) || !value.is_ascii() {
        return Err("Flow access token must be a concrete bearer credential of at most 8192 ASCII bytes".into());
    }
    if value.starts_with("opr_") {
        return Err(
            "--access-token is for the current global user; workspace bot tokens keep the bot configuration".into(),
        );
    }
    Ok(Secret::new(value))
}

impl FlowRuntime {
    pub fn load(
        config_path: Option<&Path>,
        api_url: Option<&str>,
        token: Option<&Secret>,
        token_file: Option<&Path>,
        transport_override: Option<McpTransport>,
        bind_addr: Option<&str>,
    ) -> Result<Self, String> {
        if token.is_some() && token_file.is_some() {
            return Err("Choose exactly one of --access-token and --access-token-file".into());
        }
        let config = config_path
            .map(|path| OpenPrConfig::load(Some(path)))
            .transpose()
            .map_err(|error| format!("Flow configuration error: {error}"))?;
        let configured = config.as_ref().map(|value| &value.mcp);
        let transport = transport_override
            .or_else(|| configured.map(|value| value.transport))
            .unwrap_or(McpTransport::Stdio);
        let api_url = api_url
            .map(str::to_owned)
            .or_else(|| configured.and_then(|value| value.api_url.clone()))
            .unwrap_or_else(|| platform::config::DEFAULT_MCP_API_URL.into());
        let parsed =
            reqwest::Url::parse(&api_url).map_err(|_| "Flow API URL must be a concrete HTTP(S) URL".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err("Flow API URL must be HTTP(S), without embedded credentials".into());
        }
        let credential = match token_file {
            Some(path) => {
                let metadata = std::fs::metadata(path).map_err(|_| "Cannot read Flow access-token file".to_string())?;
                if metadata.len() > 8192 {
                    return Err("Flow access-token file exceeds 8192 bytes".into());
                }
                Some(access_token(
                    &std::fs::read_to_string(path).map_err(|_| "Cannot read Flow access-token file".to_string())?,
                )?)
            }
            None => token.map(|value| access_token(value.expose())).transpose()?,
        };
        if transport.is_networked() && credential.is_some() {
            return Err(
                "Flow HTTP/SSE always use the request's Authorization bearer; do not configure a shared access token"
                    .into(),
            );
        }
        if !transport.is_networked() && credential.is_none() {
            return Err("Flow stdio/CLI requires explicit --access-token or --access-token-file; mcp.bot_token is never reinterpreted".into());
        }
        Ok(Self {
            api_url,
            workspace_id: configured.and_then(|value| value.workspace_id),
            transport,
            bind_addr: bind_addr
                .map(str::to_owned)
                .or_else(|| configured.and_then(|value| value.bind_addr.clone()))
                .unwrap_or_else(|| "127.0.0.1:8090".into()),
            credential,
            allow_write_call: configured.and_then(|value| value.allow_write_call).unwrap_or(true),
        })
    }
    pub fn client(&self) -> Result<OpenPrClient, String> {
        OpenPrClient::new(ClientConfig {
            base_url: self.api_url.clone(),
            credential: self.credential.clone(),
            workspace_id: self.workspace_id.map(|id| id.to_string()),
            transport_label: transport_label(self.transport),
        })
        .map(OpenPrClient::flow_only)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::FlowRuntime;
    use platform::config::{DEFAULT_MCP_API_URL, Secret};

    /// Without a file and without `--api-url` the Flow-only profile talks to the address the stack
    /// publishes, the same default the rest of the MCP configuration has.
    #[test]
    fn the_default_api_url_is_the_published_address() {
        let token = Secret::new("a-global-user-token");
        let runtime = FlowRuntime::load(None, None, Some(&token), None, None, None).expect("loads");
        assert_eq!(runtime.api_url, DEFAULT_MCP_API_URL);
        assert_eq!(DEFAULT_MCP_API_URL, "http://localhost:8081");
        let explicit = FlowRuntime::load(
            None,
            Some("https://sylvode.example.net"),
            Some(&token),
            None,
            None,
            None,
        )
        .expect("loads");
        assert_eq!(explicit.api_url, "https://sylvode.example.net");
    }
}
