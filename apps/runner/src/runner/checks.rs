//! The synchronous checks of a delivery to a `runner` agent (design section 7.1), in order:
//! the protocol field, the task view fetched with the task bot's token, the cross-check of project
//! id and key, execution enabled, the project key and its key registry, the working
//! directory against the allowed roots, and the channel and CLI prerequisites.
//!
//! A refusal carries a code from the design's list and a one-line reason without any path; the
//! path goes to the runner log at `info`.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use sylvode_prereq::{Checker, InstallKind};

use super::config::RunnerConfig;
use super::files::{BotFiles, StateDir};
use super::paths::AllowedRoots;
use super::registry::SessionRegistry;
use super::sv_client::{SvClient, SvError, TASK_PROTOCOL, token_refused};
use crate::config::{AiTaskCallbackTarget, Config};

/// The credentials of the bot a delivery is for, to write its MCP files. `Debug` hides the token.
#[derive(Clone)]
pub struct BotCreds {
    pub bot_id: String,
    pub api_url: String,
    /// The workspace the bot belongs to: the MCP server needs it to start.
    pub workspace_id: String,
    pub token: String,
}

impl BotCreds {
    /// The credentials of `bot` from its resolved `[ai_task_callback]` target. `None` when the
    /// target has no workspace (a runner bot always has one: the configuration check refuses it
    /// otherwise).
    pub fn for_target(bot: &str, target: &AiTaskCallbackTarget<'_>) -> Option<Self> {
        Some(Self {
            bot_id: bot.to_string(),
            api_url: target.api_url.to_string(),
            workspace_id: target.workspace_id?.to_string(),
            token: target.token.to_string(),
        })
    }

    /// What goes into this bot's MCP file: the one place the file's settings are decided, for the
    /// interactive and the headless channel alike.
    pub fn mcp_files<'a>(&'a self, runner: &'a RunnerConfig) -> BotFiles<'a> {
        BotFiles {
            bot_id: &self.bot_id,
            api_url: &self.api_url,
            workspace_id: &self.workspace_id,
            token: &self.token,
            tool_mode: &runner.mcp.tool_mode,
            allow_write_call: runner.mcp.allow_write_call,
        }
    }
}

impl std::fmt::Debug for BotCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotCreds")
            .field("bot_id", &self.bot_id)
            .field("api_url", &self.api_url)
            .field("workspace_id", &self.workspace_id)
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
    pub(super) fn new(code: &'static str, reason: impl Into<String>) -> Self {
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
    /// The slot of the task: the project's own (`<KEY>-<cli>`) or, when this runner configures a
    /// session for the bot and CLI, the bot's.
    pub slot: super::tmux::Slot,
    /// The tmux session of a tmux task, and where its name came from (derived for a headless one).
    pub tmux: super::tmux::TmuxName,
    pub name_source: super::tmux::NameSource,
    #[allow(dead_code, reason = "the session manager reads it")]
    pub project_id: String,
    /// The task message of design section 4.9, built from the task view.
    pub message: String,
    /// The canonical working directory; the only form of the path used from here on.
    pub dir: PathBuf,
    /// The directory passed the allowed roots (always, unless the tmux session already exists or
    /// the slot is a bot's, which never uses a directory).
    pub dir_checked: bool,
    pub channel: Channel,
    /// `claude`, `codex` or `opencode`.
    pub cli: String,
    /// The program that runs the CLI, as found on the `PATH` or given as `binary`.
    pub program: PathBuf,
    /// The MCP server executable.
    pub mcp_server: PathBuf,
}

impl Accepted {
    /// Whether the task is typed into a tmux session: the tmux channel, or a bot's own session
    /// whatever the channel says.
    pub fn uses_tmux(&self) -> bool {
        self.channel == Channel::Tmux || self.slot.is_bot()
    }
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

/// Whether `key` is a valid project key (the first part of a session name): `^[A-Z0-9]{1,32}$`.
pub fn valid_project_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 32 && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// What the checks need from the running service.
pub struct Context<'a> {
    pub config: &'a Config,
    pub roots: &'a Result<AllowedRoots, String>,
    pub registry: &'a SessionRegistry,
    /// The runner's state directory, when it is usable: the records of the sessions.
    pub state: Option<&'a StateDir>,
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
    // 5. The key is part of a session name and of the task text.
    if !valid_project_key(project_key) {
        return refuse("invalid_project_key", "the project key is not a valid session name");
    }
    // 6. Channel and CLI.
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
    // A bot with a configured session is served by it on either channel.
    let bot_session = runner.bot_session(&ctx.bot.bot_id, &cli).is_some();
    // A project slot's key is bound to one project; a bot slot binds no key.
    if !bot_session && ctx.registry.check(project_id, project_key).is_err() {
        return refuse(
            "session_name_collision",
            "another project already uses this session name on this runner",
        );
    }
    let tmux = if channel == Channel::Tmux || bot_session {
        // opencode has no verified tmux behaviour (readiness, dialogs, permissions): refused
        // whatever else is set.
        if !super::profile::profile(&cli).is_some_and(|p| p.tmux) {
            return refuse(
                "cli_unverified",
                format!("the {cli} CLI is not verified for the tmux channel; use the headless channel"),
            );
        }
        match tmux_prerequisites(runner).await {
            Ok(tmux) => Some(tmux),
            Err(refusal) => return Verdict::Refused(refusal),
        }
    } else {
        None
    };
    // 7. The working directory. A session that exists is used as it is, wherever its CLI runs, so
    // the allowed roots decide only when the runner would have to create the session (and always
    // for a headless run). A bot's session is never created and its directory is never read, used
    // or checked. A directory that cannot be used for an existing session is carried on as the
    // path given, and marked as not checked: should the session be gone when the task is served,
    // the runner declines to create it.
    let raw_dir = str_at(execution, &["working_directory"]).unwrap_or_default();
    let mut resolved = None;
    let mut session_exists = false;
    if let Some(tmux) = &tmux {
        let found = match super::naming::resolve(runner, &ctx.bot.bot_id, project_key, &cli) {
            Ok(found) => found,
            Err(refusal) => return Verdict::Refused(refusal),
        };
        if !found.slot.is_bot() {
            let tmux = tmux.clone();
            let target = found.tmux.clone();
            session_exists = match tokio::task::spawn_blocking(move || tmux.has_session(&target)).await {
                Ok(Ok(exists)) => exists,
                _ => return refuse("tmux_unavailable", "the tmux session could not be looked up"),
            };
            if session_exists && serves_another_bot(ctx.state, &found.slot, &ctx.bot.bot_id) {
                return refuse(
                    "session_bot_conflict",
                    format!(
                        "tmux session {} on this runner serves another bot; give this bot its own session in \
                         runner.tmux.sessions",
                        found.tmux.as_str()
                    ),
                );
            }
        }
        resolved = Some(found);
    }
    let bot_slot = resolved.as_ref().is_some_and(|found| found.slot.is_bot());
    let (dir, dir_checked) = if bot_slot {
        (PathBuf::from(raw_dir), false)
    } else {
        match roots.check_directory(raw_dir) {
            Ok(dir) => (dir, true),
            Err(_) if session_exists => (PathBuf::from(raw_dir), false),
            Err(refusal) => {
                tracing::info!(code = refusal.code, directory = ?raw_dir, "working directory declined");
                return refuse(refusal.code, refusal.reason);
            }
        }
    };
    tracing::info!(directory = ?dir, checked = dir_checked, "working directory");
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
    let found = match resolved {
        Some(found) => found,
        None => match super::naming::resolve(runner, &ctx.bot.bot_id, project_key, &cli) {
            Ok(found) => found,
            Err(_) => return refuse("cli_unavailable", "the project's CLI is not known to this runner"),
        },
    };
    let Some(message) = task_message(&view, project_key, task_id, attempt, max_attempts) else {
        return refuse(
            "delivery_mismatch",
            "the task view cannot be turned into a task message",
        );
    };
    // Every check passed: only now does the project own its key (a bot slot binds none).
    if !found.slot.is_bot() && ctx.registry.bind(project_id, project_key).is_err() {
        return refuse(
            "session_name_collision",
            "another project already uses this session name on this runner",
        );
    }
    Verdict::Run(Box::new(Accepted {
        client: ctx.client,
        bot: ctx.bot,
        task_id: task_id.to_string(),
        attempt,
        max_attempts,
        project_key: project_key.to_string(),
        slot: found.slot,
        tmux: found.tmux,
        name_source: found.source,
        project_id: project_id.to_string(),
        message,
        dir,
        dir_checked,
        channel,
        cli,
        program,
        mcp_server,
    }))
}

/// Whether the record of project slot `slot` names a bot other than `bot`.
fn serves_another_bot(state: Option<&StateDir>, slot: &super::tmux::Slot, bot: &str) -> bool {
    state
        .and_then(|state| state.read_session_record(slot))
        .is_some_and(|record| !record.bot_id.is_empty() && !record.bot_id.eq_ignore_ascii_case(bot.trim()))
}

fn refuse(code: &'static str, reason: impl Into<String>) -> Verdict {
    Verdict::Refused(Refusal::new(code, reason))
}

/// tmux must be on this runner and at least the configured minimum (never below 3.2). The driver
/// is returned for the lookups that follow.
async fn tmux_prerequisites(runner: &super::config::RunnerConfig) -> Result<super::tmux::Tmux, Refusal> {
    use sylvode_prereq::probe::parse_tmux_version;
    let unavailable = |reason: &str| Refusal::new("tmux_unavailable", reason);
    let Some(program) = Checker::new(InstallKind::RunnerTmux).locate(&runner.tmux.binary) else {
        return Err(unavailable("tmux was not found on this runner"));
    };
    let socket = super::tmux::SocketName::new(&runner.tmux.socket_name)
        .map_err(|_| unavailable("the tmux socket name is not valid"))?;
    let tmux = super::tmux::Tmux::new(socket).with_program(program);
    let probe = tmux.clone();
    let version = tokio::task::spawn_blocking(move || probe.version())
        .await
        .map_err(|_| unavailable("the tmux version could not be read"))?
        .ok();
    let Some(parsed) = version.as_deref().and_then(parse_tmux_version) else {
        return Err(unavailable("the tmux version could not be read"));
    };
    let min = runner
        .tmux
        .min_version
        .split_once('.')
        .and_then(|(major, minor)| Some((major.parse().ok()?, minor.parse().ok()?)))
        .ok_or_else(|| unavailable("the configured tmux minimum is not valid"))?;
    if !parsed.meets(min) {
        return Err(Refusal::new(
            "tmux_unavailable",
            format!("tmux {} is older than {}", parsed.display, runner.tmux.min_version),
        ));
    }
    Ok(tmux)
}

/// The task message of design section 4.9 for the view of a task: the issue or proposal it is
/// about, the title (sanitised by the builder), the attempt. `None` when the view holds a value the
/// builder refuses.
pub fn task_message(view: &Value, project_key: &str, task_id: &str, attempt: i64, max_attempts: i64) -> Option<String> {
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
        project_key,
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
