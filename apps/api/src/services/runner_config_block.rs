//! The runner configuration block: the TOML text a person saves as the runner's configuration
//! file after connecting a bot.
//!
//! The text is the contract with the stage-5 runner's configuration loader. It is built from values
//! held in memory, returned once inside the connect or rotate answer, and never stored: it holds
//! the bot's token and the webhook's signing secret. Nothing here logs, formats into an error, or
//! derives `Debug` over the text.

use std::fmt::Write as _;

use platform::timestamp::Timestamp;
use serde::Serialize;
use uuid::Uuid;

/// `config_block.format`: the version of the text below.
pub const FORMAT: &str = "sylvode-runner-toml-1";
/// `config_block.filename`: what the person is told to save it as.
pub const FILENAME: &str = "sylvode-runner.toml";
/// `api_url` when neither `[oauth] issuer` nor a request origin is known. The runner refuses it at
/// start-up, so a missing value is loud.
pub const API_URL_PLACEHOLDER: &str = "replace-with-the-sylvode-api-url";
/// `listen` when the runner address does not name a local interface.
const DEFAULT_LISTEN: &str = "127.0.0.1:9090";

/// The answer's `config_block`.
#[derive(Serialize)]
pub struct ConfigBlock {
    pub format: &'static str,
    pub filename: &'static str,
    pub bot_id: Uuid,
    pub token_prefix: String,
    pub issued_at: String,
    /// The TOML text. Holds the token and the signing secret.
    pub text: String,
}

impl std::fmt::Debug for ConfigBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigBlock")
            .field("bot_id", &self.bot_id)
            .field("token_prefix", &self.token_prefix)
            .field("issued_at", &self.issued_at)
            .field("text", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// What the text is built from.
pub struct BlockInput<'a> {
    pub bot_name: &'a str,
    pub bot_id: Uuid,
    pub project_key: &'a str,
    pub issued_at: Timestamp,
    /// The runner address as stored (no trailing slash).
    pub runner_url: &'a str,
    pub working_directory: &'a str,
    pub cli: &'a str,
    pub token: &'a str,
    pub token_prefix: &'a str,
    pub secret: &'a str,
    /// `[oauth] issuer` when configured.
    pub issuer: Option<&'a str>,
    /// The request's `Origin` header, used when there is no issuer.
    pub origin: Option<&'a str>,
}

/// A TOML basic string: `"` and `\` escaped, control characters written as `\uXXXX`. Validation
/// refuses control characters upstream; this keeps the text valid TOML regardless.
pub fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A comment line cannot carry a line break; anything that would end it is replaced.
fn comment_safe(value: &str) -> String {
    value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// `listen` for a runner address, and whether it was taken from the address.
///
/// The address's `host:port` when the host is a literal IP or `localhost` (with the scheme's
/// default port when none is given); otherwise `127.0.0.1:9090`.
pub fn listen_for(runner_url: &str) -> (String, bool) {
    let derived = reqwest::Url::parse(runner_url).ok().and_then(|url| {
        let host = url.host_str()?;
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let local = host.eq_ignore_ascii_case("localhost") || bare.parse::<std::net::IpAddr>().is_ok();
        if !local {
            return None;
        }
        Some(format!("{host}:{}", url.port_or_known_default()?))
    });
    derived.map_or_else(|| (DEFAULT_LISTEN.to_string(), false), |listen| (listen, true))
}

/// `api_url`: the issuer when configured, otherwise the request's `http(s)` origin, otherwise the
/// placeholder.
pub fn api_url(issuer: Option<&str>, origin: Option<&str>) -> String {
    if let Some(issuer) = issuer.map(str::trim).filter(|issuer| !issuer.is_empty()) {
        return issuer.trim_end_matches('/').to_string();
    }
    origin
        .and_then(|origin| reqwest::Url::parse(origin.trim()).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .map_or_else(
            || API_URL_PLACEHOLDER.to_string(),
            |url| url.origin().ascii_serialization(),
        )
}

/// Builds the block.
pub fn build(input: &BlockInput<'_>) -> ConfigBlock {
    let (listen, _) = listen_for(input.runner_url);
    let api_url = api_url(input.issuer, input.origin);
    let bot_id = input.bot_id;
    let name = comment_safe(input.bot_name);
    let project = comment_safe(input.project_key);
    let issued = input.issued_at.to_rfc3339();

    let mut text = String::new();
    let _ = write!(
        text,
        "# Sylvode runner configuration for bot \"{name}\" ({bot_id}),\n\
         # issued {issued} for project {project}. Shown once: it holds the bot's token and the\n\
         # signing secret. Store it with mode 0600. To add this bot to a runner that already serves\n\
         # another bot, add the secret to [security] webhook_secrets and append the [[ai_task_callback.bots]]\n\
         # and [[agents]] tables; keep the other sections once. The token acts only in projects this\n\
         # bot is connected to and has enabled, and only on the routes the runner needs.\n\n"
    );
    text.push_str("[server]\n");
    let url = comment_safe(input.runner_url);
    let _ = write!(
        text,
        "# Sylvode delivers to {url}/webhook. Listen where that address reaches\n\
         # (behind a TLS proxy, usually loopback).\n"
    );
    let _ = write!(text, "listen = {}\n\n", toml_string(&listen));
    let _ = write!(
        text,
        "[security]\nwebhook_secrets = [{}]\n\n",
        toml_string(input.secret)
    );
    let _ = write!(
        text,
        "[ai_task_callback]\nenabled = true\napi_url = {}\n\n",
        toml_string(&api_url)
    );
    let _ = write!(
        text,
        "[[ai_task_callback.bots]]\nbot_id = {}\ntoken = {}\nsurface = \"mcp_stdio\"\n\n",
        toml_string(&bot_id.to_string()),
        toml_string(input.token)
    );
    let _ = write!(
        text,
        "[runner]\n# The runner refuses any working directory outside these. Review before use.\nallowed_roots = [{}]\n\n",
        toml_string(input.working_directory)
    );
    if matches!(input.cli, "codex" | "opencode") {
        let _ = write!(
            text,
            "[runner.cli.{cli}]\n# {cli} is experimental on the tmux channel.\nenabled = true\n\n",
            cli = input.cli
        );
    }
    let _ = write!(
        text,
        "[[agents]]\nid = {id}\nname = {name}\nagent_type = \"runner\"\n\n[agents.route]\nbot_ids = [{id}]\n",
        id = toml_string(&bot_id.to_string()),
        name = toml_string(input.bot_name)
    );

    ConfigBlock {
        format: FORMAT,
        filename: FILENAME,
        bot_id,
        token_prefix: input.token_prefix.to_string(),
        issued_at: issued,
        text,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{API_URL_PLACEHOLDER, BlockInput, api_url, build, listen_for, toml_string};
    use platform::timestamp::Timestamp;
    use uuid::Uuid;

    const BOT: &str = "00000000-0000-0000-0000-000000000000";
    const GOLDEN_TOKEN: &str = "opr_golden_fixture_token";
    const GOLDEN_SECRET: &str = "golden-fixture-signing-secret";

    fn token() -> String {
        format!("opr_{}", "ab".repeat(32))
    }

    fn secret() -> String {
        "S3cretS3cretS3cretS3cretS3cretS3cretS3cretS3cretS3cretS3cretS3cr".to_string()
    }

    fn input<'a>(token: &'a str, secret: &'a str, name: &'a str, url: &'a str, cli: &'a str) -> BlockInput<'a> {
        BlockInput {
            bot_name: name,
            bot_id: Uuid::parse_str(BOT).expect("uuid"),
            project_key: "SVWL",
            issued_at: Timestamp::parse_rfc3339("2026-11-21T10:00:00Z").expect("time"),
            runner_url: url,
            working_directory: "/home/dev/work/svwl",
            cli,
            token,
            token_prefix: "opr_abab",
            secret,
            issuer: Some("https://sylvode.example.net"),
            origin: None,
        }
    }

    fn parse(text: &str) -> toml::Table {
        text.parse::<toml::Table>().expect("the block is valid TOML")
    }

    /// The block is the text the stage-5 runner reads. The golden file pins it byte for byte; set
    /// `UPDATE_RUNNER_BLOCK_GOLDEN=1` to rewrite it after an intended change.
    #[test]
    fn the_block_text_matches_the_golden_file() {
        // Fixed placeholders that are plainly not credentials: a committed `opr_` followed by 20 or
        // more letters and digits is refused by `scripts/test-no-instance-literals.sh`, and the
        // runner's loader accepts these as usable (they carry none of its placeholder markers).
        let block = build(&input(
            GOLDEN_TOKEN,
            GOLDEN_SECRET,
            "Build bot",
            "https://runner.example.net:9090",
            "claude",
        ));
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/runner-config-block.toml");
        if std::env::var_os("UPDATE_RUNNER_BLOCK_GOLDEN").is_some() {
            std::fs::write(path, &block.text).expect("the golden file is written");
        }
        let golden = std::fs::read_to_string(path).expect("the golden file exists");
        assert_eq!(
            block.text, golden,
            "the block text changed: the runner's loader reads this text"
        );
        assert_eq!(block.format, "sylvode-runner-toml-1");
        assert_eq!(block.filename, "sylvode-runner.toml");
    }

    #[test]
    fn the_block_has_the_keys_the_runner_reads_and_no_participant_fields() {
        let (token, secret) = (token(), secret());
        let block = build(&input(
            &token,
            &secret,
            "Build bot",
            "https://runner.example.net:9090",
            "claude",
        ));
        let table = parse(&block.text);
        assert_eq!(table["server"]["listen"].as_str(), Some("127.0.0.1:9090"));
        assert_eq!(table["security"]["webhook_secrets"][0].as_str(), Some(secret.as_str()));
        let callback = &table["ai_task_callback"];
        assert_eq!(callback["enabled"].as_bool(), Some(true));
        assert_eq!(callback["api_url"].as_str(), Some("https://sylvode.example.net"));
        let bots = callback["bots"].as_array().expect("bots");
        assert_eq!(bots.len(), 1);
        assert_eq!(bots[0]["bot_id"].as_str(), Some(BOT));
        assert_eq!(bots[0]["token"].as_str(), Some(token.as_str()));
        assert_eq!(bots[0]["surface"].as_str(), Some("mcp_stdio"));
        assert_eq!(
            table["runner"]["allowed_roots"][0].as_str(),
            Some("/home/dev/work/svwl")
        );
        let agents = table["agents"].as_array().expect("agents");
        assert_eq!(agents[0]["agent_type"].as_str(), Some("runner"));
        assert_eq!(agents[0]["id"].as_str(), Some(BOT));
        assert_eq!(agents[0]["route"]["bot_ids"][0].as_str(), Some(BOT));
        assert!(table["runner"].get("cli").is_none(), "no cli section for claude");
        for word in ["capabilities", "model", "provider"] {
            assert!(!block.text.contains(word), "the block must not mention {word}");
        }
    }

    #[test]
    fn names_with_quotes_and_backslashes_stay_valid_toml() {
        let (token, secret) = (token(), secret());
        let name = r#"Bot "A" \ B"#;
        let block = build(&input(
            &token,
            &secret,
            name,
            "https://runner.example.net:9090",
            "claude",
        ));
        let table = parse(&block.text);
        assert_eq!(table["agents"][0]["name"].as_str(), Some(name));
        assert_eq!(toml_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000A\"");
        // A name with a line break cannot break out of the comment lines either.
        let block = build(&input(
            &token,
            &secret,
            "x\n[evil]\nk = 1",
            "https://runner.example.net:9090",
            "claude",
        ));
        assert!(parse(&block.text).get("evil").is_none());
    }

    #[test]
    fn listen_follows_the_address_only_for_local_interfaces() {
        assert_eq!(
            listen_for("http://127.0.0.1:8080"),
            ("127.0.0.1:8080".to_string(), true)
        );
        assert_eq!(listen_for("http://localhost"), ("localhost:80".to_string(), true));
        assert_eq!(listen_for("https://localhost"), ("localhost:443".to_string(), true));
        assert_eq!(listen_for("http://[::1]:9000"), ("[::1]:9000".to_string(), true));
        assert_eq!(
            listen_for("https://runner.example.net:9090"),
            ("127.0.0.1:9090".to_string(), false)
        );
        assert_eq!(
            listen_for("https://runner.example.net"),
            ("127.0.0.1:9090".to_string(), false)
        );
    }

    #[test]
    fn api_url_comes_from_the_issuer_then_the_origin_then_a_loud_placeholder() {
        assert_eq!(
            api_url(Some("https://sv.example.net/"), Some("https://other")),
            "https://sv.example.net"
        );
        assert_eq!(api_url(None, Some("http://127.0.0.1:18097")), "http://127.0.0.1:18097");
        assert_eq!(api_url(None, Some("https://sv.example.net")), "https://sv.example.net");
        assert_eq!(api_url(None, Some("file:///etc")), API_URL_PLACEHOLDER);
        assert_eq!(api_url(None, Some("null")), API_URL_PLACEHOLDER);
        assert_eq!(api_url(None, None), API_URL_PLACEHOLDER);
    }

    #[test]
    fn codex_and_opencode_add_their_experimental_section() {
        let (token, secret) = (token(), secret());
        for cli in ["codex", "opencode"] {
            let block = build(&input(&token, &secret, "Build bot", "http://127.0.0.1:9090", cli));
            let table = parse(&block.text);
            assert_eq!(table["runner"]["cli"][cli]["enabled"].as_bool(), Some(true), "{cli}");
            assert!(block.text.contains("experimental on the tmux channel"));
            assert_eq!(table["server"]["listen"].as_str(), Some("127.0.0.1:9090"));
        }
    }

    #[test]
    fn debug_never_prints_the_text() {
        let (token, secret) = (token(), secret());
        let block = build(&input(&token, &secret, "Build bot", "http://127.0.0.1:9090", "claude"));
        let printed = format!("{block:?}");
        assert!(!printed.contains(&token) && !printed.contains(&secret));
    }

    /// For strings full of characters that end or confuse a TOML string, comment or table (quotes,
    /// backslashes, every kind of line break, DEL, the bidirectional override, `]]`, triple
    /// quotes) in the bot name, project key, directory and runner address, the block is valid TOML,
    /// has the same keys as the golden block, and the name and directory read back exactly.
    #[test]
    fn hostile_values_keep_the_block_valid_and_its_keys_unchanged() {
        const PIECES: [&str; 16] = [
            "\"",
            "\\",
            "\r",
            "\n",
            "\t",
            "\u{7f}",
            "\u{2028}",
            "\u{202e}",
            "]]",
            "'''",
            "\"\"\"",
            "#",
            " ",
            "a",
            "\n[agents]\nid = \"x\"\n",
            "\u{0}",
        ];
        fn keys(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
            for (key, value) in table {
                let path = format!("{prefix}{key}");
                match value {
                    toml::Value::Table(inner) => keys(inner, &format!("{path}."), out),
                    toml::Value::Array(rows) if rows.iter().all(toml::Value::is_table) => {
                        for inner in rows.iter().filter_map(toml::Value::as_table) {
                            keys(inner, &format!("{path}[]."), out);
                        }
                    }
                    _ => out.push(path),
                }
            }
        }
        let (token, secret) = (token(), secret());
        let golden = parse(
            &build(&input(
                &token,
                &secret,
                "Build bot",
                "https://runner.example.net:9090",
                "claude",
            ))
            .text,
        );
        let mut golden_keys = Vec::new();
        keys(&golden, "", &mut golden_keys);
        golden_keys.sort();

        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let nasty = |next: &mut dyn FnMut() -> u64| {
            let count = 1 + usize::try_from(next() % 8).expect("small");
            (0..count)
                .map(|_| PIECES[usize::try_from(next() % 16).expect("small")])
                .collect::<String>()
        };
        for round in 0..400 {
            let name = nasty(&mut next);
            let directory = format!("/home/{}", nasty(&mut next));
            let project = nasty(&mut next);
            let url = format!("https://h{}", nasty(&mut next));
            let mut block_input = input(&token, &secret, &name, "https://runner.example.net:9090", "claude");
            block_input.working_directory = &directory;
            block_input.project_key = &project;
            block_input.runner_url = &url;
            let text = build(&block_input).text;
            let table = text.parse::<toml::Table>().unwrap_or_else(|err| {
                panic!("round {round}: not TOML ({err}) for {name:?} {directory:?} {project:?} {url:?}")
            });
            let mut found = Vec::new();
            keys(&table, "", &mut found);
            found.sort();
            assert_eq!(found, golden_keys, "round {round}: the key set changed for {name:?}");
            assert_eq!(
                table["agents"][0]["name"].as_str(),
                Some(name.as_str()),
                "round {round}"
            );
            assert_eq!(
                table["runner"]["allowed_roots"][0].as_str(),
                Some(directory.as_str()),
                "round {round}"
            );
        }
    }
}
