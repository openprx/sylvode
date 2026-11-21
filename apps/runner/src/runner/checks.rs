//! The synchronous checks of a delivery to a `runner` agent (design section 7.1), in order:
//! the protocol field, the task view fetched with the task bot's token, the cross-check of project
//! id and key, execution enabled, the project key and its session-name registry, the working
//! directory against the allowed roots, and the channel and CLI prerequisites.
//!
//! A refusal carries a code from the design's list and a one-line reason without any path; the
//! path goes to the runner log at `info`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use sylvode_prereq::{Checker, InstallKind};

use super::paths::AllowedRoots;
use super::sv_client::{SvClient, SvError, TASK_PROTOCOL, token_refused};
use crate::config::Config;

/// The credentials of the bot a delivery is for, to write its MCP files. `Debug` hides the token.
#[derive(Clone)]
pub struct BotCreds {
    pub bot_id: String,
    pub api_url: String,
    pub token: String,
}

impl std::fmt::Debug for BotCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotCreds")
            .field("bot_id", &self.bot_id)
            .field("api_url", &self.api_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// A refused delivery.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub reason: String,
}

impl Refusal {
    fn new(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

/// The channel of a project's execution settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Tmux,
    Headless,
}

/// A delivery that passed every check.
#[derive(Debug)]
pub struct Accepted {
    pub client: SvClient,
    pub bot: BotCreds,
    pub task_id: String,
    pub attempt: i64,
    #[allow(dead_code, reason = "the session manager reads it")]
    pub max_attempts: i64,
    pub project_key: String,
    #[allow(dead_code, reason = "the session manager reads it")]
    pub project_id: String,
    /// The task message of design section 4.9, built from the task view.
    pub message: String,
    /// The canonical working directory; the only form of the path used from here on.
    pub dir: PathBuf,
    pub channel: Channel,
    /// `claude`, `codex` or `opencode`.
    pub cli: String,
    /// The program that runs the CLI, as found on the `PATH` or given as `binary`.
    pub program: PathBuf,
    /// The MCP server executable.
    pub mcp_server: PathBuf,
}

/// What the checks decided.
#[derive(Debug)]
pub enum Verdict {
    Run(Box<Accepted>),
    Refused(Refusal),
    /// The attempt is no longer active in Sylvode.
    Ignored(String),
    /// Sylvode could not answer in time: answer HTTP 503 so the worker re-queues.
    Unavailable,
}

/// Which project key a project last used, and the reverse check that a key is never shared
/// (design C16). Kept in `sessions/registry.json` under the state directory (mode 0600).
#[derive(Debug, Default)]
pub struct SessionRegistry {
    file: Option<PathBuf>,
    keys: Mutex<HashMap<String, String>>,
}

impl SessionRegistry {
    /// A registry persisted at `<state_dir>/sessions/registry.json`; an unreadable or unparsable
    /// file starts an empty registry (a warning is logged).
    pub fn open(state_dir: Option<&Path>) -> Self {
        let file = state_dir.map(|dir| dir.join("sessions").join("registry.json"));
        let keys = file
            .as_deref()
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, String>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            file,
            keys: Mutex::new(keys),
        }
    }

    /// Records that `project_id` uses `key`. A project that changes its key records the new one and
    /// leaves the old session alone; a key already bound to another project is a collision.
    ///
    /// # Errors
    ///
    /// `Err(())` when `key` is bound to a different project.
    pub fn bind(&self, project_id: &str, key: &str) -> Result<(), ()> {
        let snapshot = {
            let mut keys = self.keys.lock();
            if keys.iter().any(|(other, bound)| bound == key && other != project_id) {
                return Err(());
            }
            if keys.get(project_id).map(String::as_str) == Some(key) {
                return Ok(());
            }
            keys.insert(project_id.to_string(), key.to_string());
            keys.clone()
        };
        if let Some(file) = &self.file
            && let Err(e) = persist(file, &snapshot)
        {
            tracing::warn!("could not save the session registry: {e}");
        }
        Ok(())
    }
}

fn persist(file: &Path, keys: &HashMap<String, String>) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = file.parent().ok_or("no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.kind().to_string())?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let temp = dir.join(format!(".registry.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| e.kind().to_string())?;
        out.write_all(&serde_json::to_vec(keys).map_err(|e| e.to_string())?)
            .map_err(|e| e.kind().to_string())?;
        std::fs::rename(&temp, file).map_err(|e| e.kind().to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Whether `key` is a valid session name: `^[A-Z0-9]{1,32}$`.
pub fn valid_project_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 32 && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// What the checks need from the running service.
pub struct Context<'a> {
    pub config: &'a Config,
    pub roots: &'a Result<AllowedRoots, String>,
    pub registry: &'a SessionRegistry,
    /// `Some(text)` when the protocol handshake found a mismatch.
    pub protocol_mismatch: Option<String>,
    pub client: SvClient,
    pub bot: BotCreds,
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str()
}

/// Runs the synchronous checks for `payload`, a delivery to a `runner` agent.
pub async fn check_delivery(ctx: Context<'_>, payload: &Value) -> Verdict {
    let runner = &ctx.config.runner;
    // 1. The protocol field: a delivery without it, or with another value, is refused.
    match payload.get("protocol").and_then(Value::as_u64) {
        Some(p) if p == u64::from(TASK_PROTOCOL) => {}
        other => {
            let seen = other.map_or_else(|| "missing".to_string(), |p| p.to_string());
            return refuse(
                "protocol_mismatch",
                format!("task protocol mismatch (runner {TASK_PROTOCOL}, delivery {seen})"),
            );
        }
    }
    if let Some(text) = &ctx.protocol_mismatch {
        return refuse("protocol_mismatch", text.clone());
    }
    let roots = match ctx.roots {
        Ok(roots) => roots,
        Err(e) => {
            tracing::error!("runner agents are refused: {e}");
            return refuse("runner_agent_disabled", "the runner's allowed_roots are not valid");
        }
    };
    let (Some(task_id), Some(project_id)) = (
        str_at(payload, &["task_id"]).filter(|id| uuid::Uuid::parse_str(id).is_ok()),
        str_at(payload, &["project_id"]).filter(|id| uuid::Uuid::parse_str(id).is_ok()),
    ) else {
        return refuse("delivery_mismatch", "the delivery has no valid task id and project id");
    };
    let attempt = payload.get("attempts").and_then(Value::as_i64).unwrap_or(0);
    let max_attempts = payload.get("max_attempts").and_then(Value::as_i64).unwrap_or(0);
    // 2. The task view, with the task bot's token.
    let timeout = Duration::from_secs(runner.delivery_check_timeout_secs);
    let view = match ctx.client.task_view(task_id, attempt, timeout).await {
        Ok(view) => view,
        Err(SvError::Status(400)) => {
            return Verdict::Ignored(format!("attempt {attempt} is no longer active in Sylvode"));
        }
        Err(e) if e.is_transient() => {
            tracing::warn!("task view not available: {e}");
            return Verdict::Unavailable;
        }
        Err(e) if token_refused(&e) => {
            tracing::error!("the bot's token was refused by Sylvode while fetching a task");
            return refuse("runner_agent_disabled", "Sylvode refused the bot's token");
        }
        Err(e) => {
            tracing::warn!("task view refused: {e}");
            return refuse("delivery_mismatch", "Sylvode does not know this task for this bot");
        }
    };
    if view.get("active").and_then(Value::as_bool) == Some(false) {
        return Verdict::Ignored(format!("attempt {attempt} is no longer active in Sylvode"));
    }
    // 3. Cross-check the delivery against Sylvode's record.
    let project_key = str_at(&view, &["project", "key"]).unwrap_or_default();
    let consistent = str_at(&view, &["task_id"]) == Some(task_id)
        && str_at(&view, &["project", "id"]) == Some(project_id)
        && str_at(payload, &["project_key"]).is_none_or(|key| key == project_key);
    if !consistent {
        return refuse(
            "delivery_mismatch",
            "the delivery does not match Sylvode's record of the task",
        );
    }
    // 4. Execution settings.
    let execution = view.get("execution").filter(|e| !e.is_null());
    let Some(execution) = execution.filter(|e| e.get("enabled").and_then(Value::as_bool) == Some(true)) else {
        return refuse(
            "execution_disabled",
            "the project has no execution settings or they are disabled",
        );
    };
    // 5. The key is a session name, bound to one project.
    if !valid_project_key(project_key) {
        return refuse("invalid_project_key", "the project key is not a valid session name");
    }
    if ctx.registry.bind(project_id, project_key).is_err() {
        return refuse(
            "session_name_collision",
            "another project already uses this session name on this runner",
        );
    }
    // 6. The working directory.
    let raw_dir = str_at(execution, &["working_directory"]).unwrap_or_default();
    let dir = match roots.check_directory(raw_dir) {
        Ok(dir) => dir,
        Err(refusal) => {
            tracing::info!(code = refusal.code, directory = %raw_dir, "working directory declined");
            return refuse(refusal.code, refusal.reason);
        }
    };
    tracing::info!(directory = %dir.display(), "working directory accepted");
    // 7. Channel and CLI.
    let channel = match str_at(execution, &["channel"]) {
        Some("tmux") => Channel::Tmux,
        Some("headless") => Channel::Headless,
        _ => {
            return refuse(
                "delivery_mismatch",
                "the project's execution channel is not known to this runner",
            );
        }
    };
    let cli = str_at(execution, &["cli"]).unwrap_or_default().to_string();
    if channel == Channel::Tmux
        && let Some(refusal) = tmux_prerequisites(runner).await
    {
        return Verdict::Refused(refusal);
    }
    let (enabled, binary) = match cli.as_str() {
        "claude" => (true, &runner.cli.claude.binary),
        "codex" => (runner.cli.codex.enabled, &runner.cli.codex.binary),
        "opencode" => (runner.cli.opencode.enabled, &runner.cli.opencode.binary),
        _ => return refuse("cli_unavailable", "the project's CLI is not known to this runner"),
    };
    if !enabled {
        return refuse("cli_disabled", format!("the {cli} CLI is not enabled on this runner"));
    }
    let finder = Checker::new(InstallKind::RunnerHeadless);
    let Some(program) = finder.locate(binary) else {
        return refuse("cli_unavailable", format!("the {cli} CLI was not found on this runner"));
    };
    let Some(mcp_server) = finder.locate(&runner.mcp.server_binary) else {
        return refuse(
            "cli_unavailable",
            "the MCP server (mcp-server) was not found on this runner",
        );
    };
    let Some(message) = task_message(&view, task_id, attempt, max_attempts) else {
        return refuse(
            "delivery_mismatch",
            "the task view cannot be turned into a task message",
        );
    };
    Verdict::Run(Box::new(Accepted {
        client: ctx.client,
        bot: ctx.bot,
        task_id: task_id.to_string(),
        attempt,
        max_attempts,
        project_key: project_key.to_string(),
        project_id: project_id.to_string(),
        message,
        dir,
        channel,
        cli,
        program,
        mcp_server,
    }))
}

fn refuse(code: &'static str, reason: impl Into<String>) -> Verdict {
    Verdict::Refused(Refusal::new(code, reason))
}

/// tmux must be on this runner and at least the configured minimum (never below 3.2).
async fn tmux_prerequisites(runner: &super::config::RunnerConfig) -> Option<Refusal> {
    use sylvode_prereq::probe::parse_tmux_version;
    let Some(program) = Checker::new(InstallKind::RunnerTmux).locate(&runner.tmux.binary) else {
        return Some(Refusal::new("tmux_unavailable", "tmux was not found on this runner"));
    };
    let socket = super::tmux::SocketName::new(&runner.tmux.socket_name).ok()?;
    let tmux = super::tmux::Tmux::new(socket).with_program(program);
    let version = tokio::task::spawn_blocking(move || tmux.version()).await.ok()?.ok();
    let Some(parsed) = version.as_deref().and_then(parse_tmux_version) else {
        return Some(Refusal::new("tmux_unavailable", "the tmux version could not be read"));
    };
    let min = {
        let (major, minor) = runner.tmux.min_version.split_once('.')?;
        (major.parse().ok()?, minor.parse().ok()?)
    };
    if !parsed.meets(min) {
        return Some(Refusal::new(
            "tmux_unavailable",
            format!("tmux {} is older than {}", parsed.display, runner.tmux.min_version),
        ));
    }
    None
}

/// The task message of design section 4.9 for the view of a task: the issue or proposal it is
/// about, the title (sanitised by the builder), the attempt. `None` when the view holds a value the
/// builder refuses.
pub fn task_message(view: &Value, task_id: &str, attempt: i64, max_attempts: i64) -> Option<String> {
    use super::message::{Identifier, Source, TaskMessage};
    let issue_identifier = || str_at(view, &["source", "issue", "identifier"]).map(Identifier::parse);
    let title = str_at(view, &["source", "issue", "title"])
        .or_else(|| str_at(view, &["source", "proposal", "title"]))
        .unwrap_or_default();
    let source = match str_at(view, &["source", "kind"]) {
        Some("proposal_vote") => Source::ProposalVote {
            proposal_id: str_at(view, &["source", "proposal", "id"])
                .unwrap_or_default()
                .to_string(),
        },
        kind => {
            let identifier = issue_identifier().transpose().ok()?;
            match (kind, identifier) {
                (Some("comment_mention"), Some(id)) => Source::CommentMention(id),
                (Some("review_request"), Some(id)) => Source::ReviewRequest(id),
                (Some("issue_assignment"), Some(id)) => Source::Assignment(id),
                (_, identifier) => Source::Manual(identifier),
            }
        }
    };
    TaskMessage {
        id: uuid::Uuid::parse_str(task_id).ok()?,
        attempt: u32::try_from(attempt).ok()?,
        max_attempts: u32::try_from(max_attempts).ok()?,
        source,
        title,
        earlier_attempt: None,
    }
    .render()
    .ok()
}
