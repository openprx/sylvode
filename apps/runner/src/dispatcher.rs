use crate::ai_task::ForwardFailure;
use crate::background::Background;
use crate::process::{self, End};
use crate::{config::AgentConfig, config::Config};
use serde_json::Value;
use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;

const DEFAULT_SUBPROCESS_TIMEOUT_SECS: u64 = 60;

/// Message sent by the `openclaw`, `openprx` and `custom` agents without a `message_template`.
#[allow(clippy::literal_string_with_formatting_args)]
const DEFAULT_MESSAGE_TEMPLATE: &str = "[{project}] {event}: {key} {title}\n{actor} | Trigger: {reason}";

/// Whether the agent's work is done, and how it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed,
    /// The agent was not started and will not be on a repeat either: safe mode or a feature flag
    /// turns it off, or its configuration is incomplete. The `/webhook` answer says `refused`.
    Refused,
}

/// How a dispatch went.
#[derive(Debug, PartialEq, Eq)]
pub struct Dispatched {
    /// What the `/webhook` answer reports. The sender stores the answer in its delivery log, so
    /// this never contains command output or the body of a remote error.
    pub summary: String,
    /// The full result, for this service's own log: command output, remote error bodies.
    pub detail: String,
    pub outcome: Outcome,
}

impl Dispatched {
    fn with(outcome: Outcome, summary: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            detail: detail.into(),
            outcome,
        }
    }

    /// A result written by this service, safe to report as it is.
    fn plain(outcome: Outcome, text: impl Into<String>) -> Self {
        let text = text.into();
        Self::with(outcome, text.clone(), text)
    }

    fn ok(text: impl Into<String>) -> Self {
        Self::plain(Outcome::Succeeded, text)
    }

    fn failed(text: impl Into<String>) -> Self {
        Self::plain(Outcome::Failed, text)
    }

    fn failed_with(summary: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::with(Outcome::Failed, summary, detail)
    }

    fn refused(text: impl Into<String>) -> Self {
        Self::plain(Outcome::Refused, text)
    }
}

/// Hands the delivery to a forwarding agent. A failed hand-over ends a worker AI task through the
/// task protocol (see [`crate::ai_task`]); a successful one reports nothing, because the receiving
/// side owns the task from then on.
pub async fn dispatch(
    config: &Config,
    background: &Background,
    http: Option<&reqwest::Client>,
    agent: &AgentConfig,
    payload: &Value,
) -> Dispatched {
    let dispatched = match agent.agent_type.as_str() {
        "openclaw" => dispatch_openclaw(config, background, agent, payload).await,
        "openprx" => dispatch_openprx(config, background, agent, payload).await,
        "webhook" => dispatch_webhook(config, agent, payload).await,
        "custom" => dispatch_custom(config, background, agent, payload).await,
        "cli" => Dispatched::refused(
            "agent_type = \"cli\" was removed; use agent_type = \"runner\" with the project's channel set to headless",
        ),
        other => Dispatched::refused(format!("unknown agent type: {other}")),
    };
    if matches!(dispatched.outcome, Outcome::Failed | Outcome::Refused)
        && let Some(http) = http
        && let Some(report) = ForwardFailure::for_delivery(config, http, payload)
    {
        // An agent that was never started would not be on a retry either.
        report.report_in_background(
            dispatched.summary.clone(),
            dispatched.outcome != Outcome::Refused,
            background,
        );
    }
    dispatched
}

/// The summary of a command that exited unsuccessfully.
fn exit_summary(code: Option<i32>) -> String {
    code.map_or_else(
        || "error: the command was terminated by a signal".to_string(),
        |code| format!("error: the command exited with status {code}"),
    )
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn nested_string_field<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().filter(|value| !value.is_empty())
}

/// Whether `payload` is an AI task envelope from the Sylvode worker.
fn is_worker_task(payload: &Value) -> bool {
    string_field(payload, "task_id").is_some() && string_field(payload, "ai_participant_id").is_some()
}

/// Where the issue of a worker AI task is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerIssue {
    /// The task's top-level `reference_id`, which the worker reads from the task record.
    ReferenceId,
    /// `payload.issue_id`.
    PayloadIssueId,
}

/// The issue of a worker AI task, by `reference_type`: one row per task shape the Sylvode API
/// produces. A reference type not listed (a proposal, none, anything else) has no issue.
///
/// - `work_item`: tasks queued when an issue is created with, or reassigned to, a bot
///   (`apps/api/src/routes/issue.rs`). The issue is `reference_id`. The free-form `payload` also
///   names an issue, but a task created by hand (below) can put anything there, so it is ignored:
///   it must not point the callback, sent with the bot's token, at another issue.
/// - `comment`: tasks queued when a comment mentions a bot (`apps/api/src/routes/comment.rs`).
///   `reference_id` is the comment, not an issue, and the envelope carries the comment's issue
///   nowhere but in `payload.issue_id`, which the API sets itself to the issue the comment was
///   just inserted on. That is the only source, so it is used.
///
/// Residual trust: a project admin (or system admin) can also create a task by hand
/// (`POST /api/v1/projects/{project_id}/ai/tasks`, `apps/api/src/routes/ai_callback.rs`) with any
/// `reference_type`, any `reference_id` and any `payload`, none of them checked against the
/// project. For such a task with a `comment` reference the issue is whatever that admin wrote
/// into the payload; with a `work_item` reference it is whatever `reference_id` they wrote. Only
/// the API, when it receives the bot's callback, can check that the bot may write to the issue.
const WORKER_ISSUE_SOURCES: [(&str, WorkerIssue); 2] = [
    ("work_item", WorkerIssue::ReferenceId),
    ("comment", WorkerIssue::PayloadIssueId),
];

fn worker_task_issue_id(task: &Value) -> Option<String> {
    let reference_type = string_field(task, "reference_type")?;
    let (_, source) = WORKER_ISSUE_SOURCES
        .iter()
        .find(|(listed, _)| *listed == reference_type)?;
    let issue = match source {
        WorkerIssue::ReferenceId => string_field(task, "reference_id"),
        WorkerIssue::PayloadIssueId => nested_string_field(task, &["payload", "issue_id"]),
    };
    issue.map(ToString::to_string)
}

/// The issue a delivery is about, which is also the issue every callback for it writes to. For a
/// worker AI task see [`WORKER_ISSUE_SOURCES`].
pub fn extract_issue_id(payload: &Value) -> Option<String> {
    if is_worker_task(payload) {
        return worker_task_issue_id(payload);
    }
    let direct_issue_id = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("id"))
        .or_else(|| payload.get("payload").and_then(|p| p.get("issue_id")))
        .or_else(|| payload.get("issue_id"));

    if let Some(issue_id) = direct_issue_id {
        return issue_id
            .as_str()
            .map(ToString::to_string)
            .or_else(|| issue_id.as_i64().map(|n| n.to_string()));
    }

    let reference_type = payload.get("reference_type").and_then(Value::as_str);
    if reference_type == Some("work_item") {
        return payload
            .get("reference_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    None
}

fn extract_issue_title(payload: &Value) -> Option<&str> {
    payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("issue_title"))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get("issue_title").and_then(Value::as_str))
        .or_else(|| payload.get("title").and_then(Value::as_str))
}

fn extract_event(payload: &Value) -> Option<&str> {
    string_field(payload, "event")
        .or_else(|| string_field(payload, "event_type"))
        .or_else(|| nested_string_field(payload, &["payload", "event"]))
        .or_else(|| nested_string_field(payload, &["payload", "event_type"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "event_type"]))
        .or_else(|| string_field(payload, "task_type"))
}

fn extract_trigger_reason(payload: &Value) -> Option<&str> {
    payload
        .get("bot_context")
        .and_then(|bc| bc.get("trigger_reason"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("trigger_kind").and_then(Value::as_str))
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("trigger"))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get("task_type").and_then(Value::as_str))
}

fn extract_project_id(payload: &Value) -> Option<&str> {
    payload
        .get("project_id")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("project_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "project_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "project_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "project_id"]))
        .or_else(|| payload.get("project").and_then(|p| p.get("id")).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

fn extract_form_id(payload: &Value) -> Option<&str> {
    string_field(payload, "form_id")
        .or_else(|| nested_string_field(payload, &["payload", "form_id"]))
        .or_else(|| nested_string_field(payload, &["metadata", "form_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "form_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "form_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "form_id"]))
}

fn extract_form_key(payload: &Value) -> Option<&str> {
    string_field(payload, "form_key")
        .or_else(|| nested_string_field(payload, &["payload", "form_key"]))
        .or_else(|| nested_string_field(payload, &["metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "form_key"]))
        .or_else(|| {
            nested_string_field(payload, &["payload", "envelope", "aggregate", "type"])
                .and_then(|value| value.strip_prefix("form."))
                .filter(|value| !value.is_empty())
        })
}

fn extract_record_id(payload: &Value) -> Option<&str> {
    string_field(payload, "record_id")
        .or_else(|| nested_string_field(payload, &["payload", "record_id"]))
        .or_else(|| nested_string_field(payload, &["metadata", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "aggregate", "id"]))
}

fn issue_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    let payload_key = format!("issue_{key}");
    payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get(key))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get(&payload_key))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get(&payload_key).and_then(Value::as_str))
}

// --- Security: all subprocess dispatchers use direct args, no sh -c ---

/// The result of a command run by an `openclaw`, `openprx` or `custom` agent.
struct CommandOutput {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn run_subprocess_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    cancelled: impl Future<Output = ()>,
) -> Result<CommandOutput, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    let finished = process::run(
        command,
        Duration::from_secs(timeout_secs),
        process::TERMINATION_GRACE,
        cancelled,
    )
    .await
    .map_err(|e| format!("spawn {program} failed: {e}"))?;
    match finished.end {
        End::Exited(status) => Ok(CommandOutput {
            success: status.success(),
            code: status.code(),
            stdout: String::from_utf8_lossy(&finished.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&finished.stderr).into_owned(),
        }),
        End::TimedOut => Err(format!("subprocess {program} timed out after {timeout_secs}s")),
        End::Cancelled => Err(format!("subprocess {program} cancelled by shutdown")),
        End::WaitFailed(e) => Err(format!("subprocess {program} wait failed: {e}")),
    }
}

/// Arguments of an `openclaw` or `openprx` command: the configured fixed arguments, then
/// `--channel <channel> --target <target> --message <message>`.
fn channel_command_args<'a>(fixed: &'a [String], channel: &'a str, target: &'a str, message: &'a str) -> Vec<&'a str> {
    fixed
        .iter()
        .map(String::as_str)
        .chain(["--channel", channel, "--target", target, "--message", message])
        .collect()
}

async fn dispatch_openclaw(
    config: &Config,
    background: &Background,
    agent: &AgentConfig,
    payload: &Value,
) -> Dispatched {
    let Some(cfg) = &agent.openclaw else {
        return Dispatched::refused("missing openclaw config");
    };

    let message = format_message(agent, payload);
    let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

    let args = channel_command_args(&cfg.args, &cfg.channel, &cfg.target, &message);

    match run_subprocess_with_timeout(&cfg.command, &args, timeout, background.cancelled()).await {
        Ok(o) if o.success => {
            let out = o.stdout.trim();
            tracing::info!(stdout = ?out, "openclaw ok");
            Dispatched::with(Outcome::Succeeded, "ok", format!("ok: {out}"))
        }
        Ok(o) => {
            let err = o.stderr.trim();
            tracing::error!(stderr = ?err, "openclaw failed");
            Dispatched::failed_with(exit_summary(o.code), format!("error: {err}"))
        }
        Err(e) => {
            tracing::error!(error = ?e, "openclaw exec failed");
            Dispatched::failed(format!("exec_error: {e}"))
        }
    }
}

async fn dispatch_openprx(
    config: &Config,
    background: &Background,
    agent: &AgentConfig,
    payload: &Value,
) -> Dispatched {
    let Some(cfg) = &agent.openprx else {
        return Dispatched::refused("missing openprx config");
    };

    let message = format_message(agent, payload);

    if let Some(signal_api) = &cfg.signal_api {
        let account = cfg.account.as_deref().unwrap_or("");
        let url = format!("{}/api/v1/send/{account}", signal_api.trim_end_matches('/'));

        let body = serde_json::json!({
            "recipients": [&cfg.target],
            "message": message
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.runtime.http_timeout_secs))
            .build();
        let Ok(client) = client else {
            return Dispatched::failed("error: failed to build http client");
        };

        return match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("openprx signal ok");
                Dispatched::ok("ok")
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::error!(%status, body = ?text, "openprx signal request refused");
                Dispatched::failed_with(
                    format!("error: the Signal API answered {status}"),
                    format!("error: {status} {text}"),
                )
            }
            Err(e) => {
                tracing::error!(error = ?e.to_string(), "openprx signal request failed");
                Dispatched::failed_with("error: the Signal API request failed", format!("error: {e}"))
            }
        };
    }

    if let Some(command) = &cfg.command {
        let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

        let args = channel_command_args(&cfg.args, &cfg.channel, &cfg.target, &message);

        return match run_subprocess_with_timeout(command, &args, timeout, background.cancelled()).await {
            Ok(o) if o.success => {
                tracing::info!("openprx cli ok");
                Dispatched::ok("ok")
            }
            Ok(o) => {
                let err = o.stderr.trim();
                tracing::error!(stderr = ?err, "openprx cli failed");
                Dispatched::failed_with(exit_summary(o.code), format!("error: {err}"))
            }
            Err(e) => Dispatched::failed(format!("exec_error: {e}")),
        };
    }

    Dispatched::refused("openprx config needs either signal_api or command")
}

fn outbound_signature_header_value(payload: &Value, secret: Option<&str>) -> Option<String> {
    secret.and_then(|secret| {
        serde_json::to_vec(payload)
            .ok()
            .and_then(|bytes| crate::signature::sign_payload(&bytes, secret).ok())
            .map(|sig| format!("sha256={sig}"))
    })
}

async fn dispatch_webhook(config: &Config, agent: &AgentConfig, payload: &Value) -> Dispatched {
    let Some(cfg) = &agent.webhook else {
        return Dispatched::refused("missing webhook config");
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.runtime.http_timeout_secs))
        .build();
    let Ok(client) = client else {
        return Dispatched::failed("webhook_error: failed to build http client");
    };

    let mut request = client.post(&cfg.url).json(payload);
    if let Some(signature_value) = outbound_signature_header_value(payload, cfg.secret.as_deref()) {
        request = request.header(crate::signature::OUTBOUND_SIGNATURE_HEADER, signature_value);
    }

    match request.send().await {
        Ok(resp) => {
            let outcome = if resp.status().is_success() {
                Outcome::Succeeded
            } else {
                Outcome::Failed
            };
            Dispatched::plain(outcome, format!("webhook: {}", resp.status()))
        }
        Err(e) => {
            tracing::warn!(error = ?e.to_string(), "webhook agent request failed");
            Dispatched::failed_with("webhook_error: the request failed", format!("webhook_error: {e}"))
        }
    }
}

async fn dispatch_custom(config: &Config, background: &Background, agent: &AgentConfig, payload: &Value) -> Dispatched {
    let Some(cfg) = &agent.custom else {
        return Dispatched::refused("missing custom config");
    };

    let message = format_message_tracked(agent, payload);
    let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

    let args = match build_custom_args(
        cfg.args.as_deref().unwrap_or_default(),
        &message.text,
        message.starts_with_value,
    ) {
        Ok(args) => args,
        Err(e) => {
            tracing::warn!(agent = ?agent.id, reason = ?e, "custom agent not run");
            return Dispatched::failed(format!("custom_error: {e}"));
        }
    };
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();

    match run_subprocess_with_timeout(&cfg.command, &args, timeout, background.cancelled()).await {
        Ok(o) => {
            if o.success {
                Dispatched::with(Outcome::Succeeded, "ok", o.stdout)
            } else {
                Dispatched::failed_with(exit_summary(o.code), o.stdout)
            }
        }
        Err(e) => Dispatched::failed(format!("custom_error: {e}")),
    }
}

const MESSAGE_PLACEHOLDER: &str = "{message}";

/// Builds the argument list of a `custom` agent.
///
/// Each configured argument containing `{message}` has it replaced by the rendered message; the
/// result stays exactly one argument (there is no shell and nothing is split). Without the
/// placeholder in any argument, the message is appended as the last argument.
///
/// An argument whose leading `-` comes from the payload is refused, unless an earlier configured
/// argument is `--`: payload text must never turn into an option of the command. A leading `-`
/// the operator wrote, in the argument (`--text={message}`) or at the start of
/// `message_template` (`- {title}`), is left alone. `message_starts_with_payload` says whether the
/// message's first character came from the payload.
fn build_custom_args<'a>(
    configured: &'a [String],
    message: &'a str,
    message_starts_with_payload: bool,
) -> Result<Vec<Cow<'a, str>>, String> {
    let has_placeholder = configured.iter().any(|arg| arg.contains(MESSAGE_PLACEHOLDER));
    let mut args = Vec::with_capacity(configured.len() + 1);
    let mut options_ended = false;
    for arg in configured {
        if arg.contains(MESSAGE_PLACEHOLDER) {
            let rendered = render_template_tracked(arg, &[("message", message)]);
            let dash_from_payload = rendered.starts_with_value && message_starts_with_payload;
            refuse_injected_option(&rendered.text, dash_from_payload, options_ended)?;
            args.push(Cow::Owned(rendered.text));
        } else {
            args.push(Cow::Borrowed(arg.as_str()));
        }
        options_ended |= arg == "--";
    }
    if !has_placeholder {
        refuse_injected_option(message, message_starts_with_payload, options_ended)?;
        args.push(Cow::Borrowed(message));
    }
    Ok(args)
}

fn refuse_injected_option(rendered: &str, starts_with_payload: bool, options_ended: bool) -> Result<(), String> {
    if !options_ended && starts_with_payload && rendered.starts_with('-') {
        return Err("the message starts with `-` and would be read as an option; \
                    put `--` before it in args if the command accepts that"
            .to_string());
    }
    Ok(())
}

fn format_message(agent: &AgentConfig, payload: &Value) -> String {
    format_message_tracked(agent, payload).text
}

/// The rendered message, and whether its first character came from the payload rather than
/// from the template the operator wrote.
fn format_message_tracked(agent: &AgentConfig, payload: &Value) -> Rendered {
    let tmpl = agent.message_template.as_deref().unwrap_or(DEFAULT_MESSAGE_TEMPLATE);

    let event = extract_event(payload).unwrap_or("unknown");
    let title = extract_issue_title(payload).unwrap_or("untitled");
    let issue_key = issue_field(payload, "key").unwrap_or("");
    let reason = extract_trigger_reason(payload).unwrap_or("unknown");
    let actor = payload
        .get("actor")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let workspace = payload
        .get("workspace")
        .and_then(|w| w.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let project = payload
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let state = issue_field(payload, "state").unwrap_or("");
    let priority = issue_field(payload, "priority").unwrap_or("");
    let issue_id = extract_issue_id(payload).unwrap_or_default();
    let project_id = extract_project_id(payload).unwrap_or("");
    let form_id = extract_form_id(payload).unwrap_or("");
    let form_key = extract_form_key(payload).unwrap_or("");
    let record_id = extract_record_id(payload).unwrap_or("");

    let url = format!("issue/{issue_id}");

    render_template_tracked(
        tmpl,
        &[
            ("event", event),
            ("title", title),
            ("key", issue_key),
            ("reason", reason),
            ("actor", actor),
            ("workspace", workspace),
            ("project", project),
            ("state", state),
            ("priority", priority),
            ("issue_id", &issue_id),
            ("project_id", project_id),
            ("form_id", form_id),
            ("form_key", form_key),
            ("record_id", record_id),
            ("url", &url),
        ],
    )
}

/// A rendered template, and where its first character came from.
struct Rendered {
    text: String,
    /// Whether the first character of `text` was inserted from a value rather than written in
    /// the template.
    starts_with_value: bool,
}

/// [`render_template`], also reporting whether the result starts with inserted text.
fn render_template_tracked(template: &str, values: &[(&str, &str)]) -> Rendered {
    let mut out = String::with_capacity(template.len());
    let mut starts_with_value = None;
    let mut push = |out: &mut String, text: &str, from_value: bool| {
        if starts_with_value.is_none() && !text.is_empty() {
            starts_with_value = Some(from_value);
        }
        out.push_str(text);
    };
    let mut rest = template;
    while let Some((before, after_open)) = rest.split_once('{') {
        push(&mut out, before, false);
        let substituted = after_open.split_once('}').and_then(|(name, after_close)| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value, after_close))
        });
        if let Some((value, after_close)) = substituted {
            push(&mut out, value, true);
            rest = after_close;
        } else {
            push(&mut out, "{", false);
            rest = after_open;
        }
    }
    push(&mut out, rest, false);
    Rendered {
        text: out,
        starts_with_value: starts_with_value.unwrap_or(false),
    }
}

#[cfg(test)]
// `serde_json::Value` indexing returns `Null` for a missing key; it does not panic.
#[allow(clippy::literal_string_with_formatting_args, clippy::indexing_slicing)]
mod tests {
    use super::{dispatch, extract_issue_id, format_message, outbound_signature_header_value};
    use crate::background::Background;
    use crate::config::Config;
    use serde_json::json;
    use std::time::Duration;

    fn base_config() -> Config {
        toml::from_str(
            r#"
[server]
listen = "127.0.0.1:9090"

[security]
allow_unsigned = true
webhook_secrets = []
"#,
        )
        .expect("parse base config")
    }

    #[test]
    fn builds_outbound_signature_header_when_secret_exists() {
        let payload = json!({"event":"issue.created","value":1});
        let header = outbound_signature_header_value(&payload, Some("top-secret"));

        assert!(header.as_deref().unwrap_or_default().starts_with("sha256="));

        let expected_sig =
            crate::signature::sign_payload(serde_json::to_vec(&payload).unwrap().as_slice(), "top-secret")
                .expect("sign should succeed");
        assert_eq!(header, Some(format!("sha256={expected_sig}")));
    }

    /// A `webhook` agent signs the body it forwards with its `secret`, and sends no signature
    /// without one.
    #[tokio::test]
    async fn webhook_agent_signs_what_it_forwards() {
        let payload = json!({"event": "issue.assigned", "n": 1});
        for secret in [Some("forward-secret"), None] {
            let mut server = crate::test_support::Recorder::start().await;
            let secret_line = secret.map_or_else(String::new, |s| format!("secret = '{s}'\n"));
            let agent: crate::config::AgentConfig = toml::from_str(&format!(
                "id = 'w'\nname = 'W'\nagent_type = 'webhook'\n[webhook]\nurl = '{}'\n{secret_line}",
                server.url
            ))
            .expect("parse agent");
            let answer = dispatch(&base_config(), &Background::new(1), None, &agent, &payload).await;
            assert_eq!(answer.summary, "webhook: 200 OK");
            let forwarded = server.next(Duration::from_secs(5)).await.expect("forwarded request");
            assert_eq!(forwarded.body, payload);
            let header = forwarded
                .headers
                .get("x-webhook-signature")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            match secret {
                Some(secret) => {
                    let expected = crate::signature::sign_payload(&forwarded.raw, secret).expect("sign");
                    assert_eq!(header, Some(format!("sha256={expected}")));
                }
                None => assert_eq!(header, None),
            }
        }
    }

    #[test]
    fn extracts_issue_id_from_payload() {
        let payload = json!({"data": {"issue": {"id": "42"}}});
        assert_eq!(extract_issue_id(&payload).as_deref(), Some("42"));
    }

    #[test]
    fn extracts_issue_id_from_worker_ai_task_envelope() {
        let payload = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-1",
            "task_type": "issue_assigned",
            "reference_type": "work_item",
            "reference_id": "issue-99",
            "payload": {
                "project_id": "project-1",
                "trigger": "issue.create"
            }
        });

        assert_eq!(extract_issue_id(&payload).as_deref(), Some("issue-99"));
    }

    /// A worker AI task envelope as `apps/worker/src/main.rs` (`dispatch_task`) builds it from an
    /// `ai_tasks` row.
    fn worker_task(
        task_type: &str,
        reference_type: Option<&str>,
        reference_id: Option<&str>,
        payload: serde_json::Value,
    ) -> serde_json::Value {
        let mut task = json!({
            "task_id": "task-1",
            "project_id": "PROJECT",
            "ai_participant_id": "bot-1",
            "ai_participant_name": "Fixer",
            "ai_participant_agent_type": "cli",
            "task_type": task_type,
            "reference_type": reference_type,
            "reference_id": reference_id,
            "attempts": 1,
            "max_attempts": 3,
            "trigger_kind": "mention"
        });
        task["payload"] = payload;
        task
    }

    /// The issue of every worker task shape the Sylvode API produces, one row per producer.
    #[test]
    fn a_worker_task_issue_id_follows_the_shape_each_producer_sends() {
        let rows = [
            // apps/api/src/routes/issue.rs, issue created with a bot assignee.
            (
                "issue created",
                worker_task(
                    "issue_assigned",
                    Some("work_item"),
                    Some("ISSUE"),
                    json!({"issue_id": "ISSUE", "project_id": "PROJECT", "assignee_id": "bot-1", "trigger": "issue.create"}),
                ),
                Some("ISSUE"),
            ),
            // apps/api/src/routes/issue.rs, issue reassigned to a bot.
            (
                "issue reassigned",
                worker_task(
                    "issue_assigned",
                    Some("work_item"),
                    Some("ISSUE"),
                    json!({"issue_id": "ISSUE", "project_id": "PROJECT", "assignee_id": "bot-1", "trigger": "issue.update_assignee"}),
                ),
                Some("ISSUE"),
            ),
            // apps/api/src/routes/comment.rs, a comment mentioning a bot: the reference is the
            // comment, the issue is in the payload.
            (
                "comment mention",
                worker_task(
                    "comment_requested",
                    Some("comment"),
                    Some("COMMENT"),
                    json!({
                        "issue_id": "ISSUE",
                        "comment_id": "COMMENT",
                        "project_id": "PROJECT",
                        "project_type": "software",
                        "bot_id": "bot-1",
                        "trigger": "comment.created",
                        "comment": {"id": "COMMENT", "body": "@Fixer please look"}
                    }),
                ),
                Some("ISSUE"),
            ),
            // apps/api/src/services/ai_task_service.rs, a proposal opened for voting.
            (
                "proposal vote",
                worker_task(
                    "vote_requested",
                    Some("proposal"),
                    Some("PROPOSAL"),
                    json!({"proposal_id": "PROPOSAL", "project_id": "PROJECT", "trigger": "proposal.start_voting"}),
                ),
                None,
            ),
            // apps/api/src/routes/ai_callback.rs, created by a project admin: any reference, any
            // payload. A work item reference wins over the payload.
            (
                "manual work item",
                worker_task(
                    "review_requested",
                    Some("work_item"),
                    Some("ISSUE"),
                    json!({"issue_id": "OTHER-ISSUE"}),
                ),
                Some("ISSUE"),
            ),
            (
                "manual proposal",
                worker_task(
                    "review_requested",
                    Some("proposal"),
                    Some("PROPOSAL"),
                    json!({"issue_id": "OTHER-ISSUE"}),
                ),
                None,
            ),
            (
                "manual without reference",
                worker_task("review_requested", None, None, json!({"issue_id": "OTHER-ISSUE"})),
                None,
            ),
            (
                "unknown reference type",
                worker_task(
                    "review_requested",
                    Some("form_record"),
                    Some("R"),
                    json!({"issue_id": "OTHER-ISSUE"}),
                ),
                None,
            ),
            (
                "comment without issue",
                worker_task(
                    "comment_requested",
                    Some("comment"),
                    Some("COMMENT"),
                    json!({"comment_id": "COMMENT"}),
                ),
                None,
            ),
        ];
        for (shape, task, expected) in rows {
            assert_eq!(extract_issue_id(&task).as_deref(), expected, "{shape}");
        }
    }

    /// Outside a worker task (a tunnel task body, say) the order is `data.issue.id`, then
    /// `payload.issue_id`, then a top-level `issue_id`.
    #[test]
    fn issue_id_precedence_outside_worker_tasks() {
        let all = json!({"data": {"issue": {"id": "A"}}, "payload": {"issue_id": "B"}, "issue_id": "C"});
        assert_eq!(extract_issue_id(&all).as_deref(), Some("A"));
        let inner = json!({"payload": {"issue_id": "B"}, "issue_id": "C"});
        assert_eq!(extract_issue_id(&inner).as_deref(), Some("B"));
        let top = json!({"issue_id": 7});
        assert_eq!(extract_issue_id(&top).as_deref(), Some("7"));
    }

    #[test]
    fn format_message_replaces_form_placeholders() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "printer"
name = "Printer"
agent_type = "webhook"
message_template = "{event} {project_id} {form_key} {record_id}"
"#,
        )
        .expect("parse agent");
        let payload = json!({
            "payload": {
                "envelope": {
                    "version": "openpr.event.v1",
                    "event_type": "print_job.created",
                    "project_id": "project-1",
                    "metadata": {
                        "form_key": "print_job",
                        "record_id": "record-1"
                    }
                }
            }
        });

        assert_eq!(
            format_message(&agent, &payload),
            "print_job.created project-1 print_job record-1"
        );
    }

    /// A delivery shaped like the one the Sylvode API sends for `issue.assigned` to a bot
    /// (`apps/api/src/webhook_trigger.rs`, `build_payload` and `fetch_issue_payload`).
    fn issue_assigned_delivery() -> serde_json::Value {
        json!({
            "id": "6f1d1f43-0b8e-4c8a-9a51-6d0e2f0c1a11",
            "event": "issue.assigned",
            "timestamp": "2026-10-05T12:00:00+00:00",
            "workspace": {"id": "2b0c6a59-3d5e-4a43-8f43-6a1c7f2b9e01", "name": "Acme"},
            "project": {"id": "8d9e2c1b-7f6a-4e5d-9c3b-2a1f0e9d8c7b", "name": "Billing", "key": "BIL"},
            "actor": {"id": "a1", "name": "Alice", "email": "alice@example.com", "entity_type": "human"},
            "data": {
                "issue": {
                    "id": "4c3b2a19-0f8e-4d7c-b6a5-948372615e0d",
                    "key": "BIL-4C3B2A19",
                    "title": "Invoice totals are rounded twice",
                    "description": "",
                    "state": "todo",
                    "priority": "high",
                    "assignee_ids": ["b0b0b0b0-0000-4000-8000-000000000001"],
                    "label_ids": [],
                    "sprint_id": null,
                    "created_at": "2026-10-05T11:00:00+00:00",
                    "updated_at": "2026-10-05T12:00:00+00:00"
                },
                "changes": {"assignee_ids": {"old": [], "new": ["b0b0b0b0-0000-4000-8000-000000000001"]}}
            },
            "bot_context": {
                "is_bot_task": true,
                "bot_id": "b0b0b0b0-0000-4000-8000-000000000001",
                "bot_name": "Fixer",
                "bot_agent_type": "cli",
                "trigger_reason": "assigned",
                "webhook_id": "0e0e0e0e-0000-4000-8000-000000000002"
            }
        })
    }

    #[test]
    fn format_message_fills_the_default_template() {
        let agent: crate::config::AgentConfig =
            toml::from_str("id = 'o1'\nname = 'OpenClaw'\nagent_type = 'openclaw'").expect("parse agent");

        assert_eq!(
            format_message(&agent, &issue_assigned_delivery()),
            "[Billing] issue.assigned: BIL-4C3B2A19 Invoice totals are rounded twice\nAlice | Trigger: assigned"
        );
    }

    /// A value taken from the payload is inserted as it is: a placeholder inside it is not
    /// expanded again, so a title cannot pull other fields into the message.
    #[test]
    fn templates_do_not_expand_placeholders_inside_payload_values() {
        let agent: crate::config::AgentConfig = toml::from_str(
            "id = 'o1'\nname = 'OpenClaw'\nagent_type = 'openclaw'\nmessage_template = '{title} by {actor}'",
        )
        .expect("parse agent");
        let mut payload = issue_assigned_delivery();
        *payload.pointer_mut("/data/issue/title").expect("fixture has a title") = json!("see {actor} and {issue_id}");

        assert_eq!(format_message(&agent, &payload), "see {actor} and {issue_id} by Alice");
    }

    fn custom_agent(args: &str, message_template: &str) -> crate::config::AgentConfig {
        custom_agent_running("printf", args, message_template)
    }

    fn custom_agent_running(command: &str, args: &str, message_template: &str) -> crate::config::AgentConfig {
        toml::from_str(&format!(
            "id = 'logger'\nname = 'Logger'\nagent_type = 'custom'\nmessage_template = '{message_template}'\n\
             [custom]\ncommand = '{command}'\nargs = {args}\n"
        ))
        .expect("parse agent")
    }

    /// A `custom` agent that prints every argument it is given as `[argument]`, one after the
    /// other. `sh` receives the configured `args` as its positional parameters, so none of them
    /// is parsed as an option: a `printf` operand of `--` is dropped by some implementations and
    /// kept by others (GNU, BSD), which makes `printf` unfit to observe it.
    /// `rest` is the TOML for the arguments that follow the fixed prefix.
    fn argv_logging_agent(rest: &str, message_template: &str) -> crate::config::AgentConfig {
        custom_agent_running(
            "sh",
            &format!(r#"['-c', 'for a in "$@"; do printf "[%s]" "$a"; done', 'sh', {rest}]"#),
            message_template,
        )
    }

    fn titled(title: &str) -> serde_json::Value {
        let mut payload = issue_assigned_delivery();
        *payload.pointer_mut("/data/issue/title").expect("fixture has a title") = json!(title);
        payload
    }

    /// `{message}` in `args` is replaced by the rendered message, and the message is then not
    /// appended a second time.
    #[tokio::test]
    async fn custom_args_substitute_the_message_placeholder() {
        let agent = custom_agent(r#"["[%s]", "{message}"]"#, "{key} {title}");
        let out = dispatch(
            &base_config(),
            &Background::new(1),
            None,
            &agent,
            &issue_assigned_delivery(),
        )
        .await
        .detail;
        assert_eq!(out, "[BIL-4C3B2A19 Invoice totals are rounded twice]");
    }

    /// Substituted text stays one argument, whatever it contains, and is never split or
    /// interpreted by a shell.
    #[tokio::test]
    async fn custom_args_keep_a_substituted_message_as_one_argument() {
        let agent = custom_agent(r#"["[%s]", "--label={message}"]"#, "{title}");
        let out = dispatch(
            &base_config(),
            &Background::new(1),
            None,
            &agent,
            &titled("a b \"c\" $(id); d"),
        )
        .await
        .detail;
        assert_eq!(out, "[--label=a b \"c\" $(id); d]");
    }

    /// Without the placeholder the message is appended as the last argument, as before.
    #[tokio::test]
    async fn custom_args_without_placeholder_append_the_message() {
        let agent = custom_agent(r#"["[%s]"]"#, "{key}");
        let out = dispatch(
            &base_config(),
            &Background::new(1),
            None,
            &agent,
            &issue_assigned_delivery(),
        )
        .await
        .detail;
        assert_eq!(out, "[BIL-4C3B2A19]");
    }

    /// Payload text cannot become an option: an argument that starts with `-` only because of
    /// the substituted message is refused unless an earlier argument is `--`.
    #[tokio::test]
    async fn custom_args_refuse_a_message_that_would_start_an_option() {
        let config = base_config();
        let payload = titled("-v x");

        let substituted = custom_agent(r#"["[%s]", "{message}"]"#, "{title}");
        let out = dispatch(&config, &Background::new(1), None, &substituted, &payload)
            .await
            .detail;
        assert!(out.starts_with("custom_error:"), "{out}");
        assert!(out.contains("starts with `-`"), "{out}");

        let appended = custom_agent(r#"["[%s]"]"#, "{title}");
        let out = dispatch(&config, &Background::new(1), None, &appended, &payload)
            .await
            .detail;
        assert!(out.starts_with("custom_error:"), "{out}");

        // Both the configured `--` and the message reach the command, verbatim and in order.
        let separated = argv_logging_agent(r#""--", "{message}""#, "{title}");
        assert_eq!(
            dispatch(&config, &Background::new(1), None, &separated, &payload)
                .await
                .detail,
            "[--][-v x]"
        );
    }

    /// A leading `-` the operator wrote in `message_template` is not payload text, so it is not
    /// refused, whether the message is substituted or appended. A `-` that comes from the
    /// payload still is, even behind placeholders that rendered empty.
    #[tokio::test]
    async fn custom_args_accept_a_dash_written_in_the_template() {
        let config = base_config();
        let payload = issue_assigned_delivery();

        let substituted = custom_agent(r#"["[%s]", "{message}"]"#, "- {key}");
        assert_eq!(
            dispatch(&config, &Background::new(1), None, &substituted, &payload)
                .await
                .detail,
            "[- BIL-4C3B2A19]"
        );
        let appended = custom_agent(r#"["[%s]"]"#, "-{key}");
        assert_eq!(
            dispatch(&config, &Background::new(1), None, &appended, &payload)
                .await
                .detail,
            "[-BIL-4C3B2A19]"
        );

        let behind_empty = custom_agent(r#"["[%s]", "{message}"]"#, "{workspace_missing}{state_missing}");
        assert_eq!(
            dispatch(&config, &Background::new(1), None, &behind_empty, &payload)
                .await
                .detail,
            "[{workspace_missing}{state_missing}]",
            "unknown placeholders stay literal"
        );
        let mut empty_project = titled("-v x");
        *empty_project
            .pointer_mut("/project/name")
            .expect("fixture has a project") = json!("");
        let from_payload = custom_agent(r#"["[%s]", "{message}"]"#, "{project}{title}");
        let out = dispatch(&config, &Background::new(1), None, &from_payload, &empty_project)
            .await
            .detail;
        assert!(out.starts_with("custom_error:"), "{out}");
    }

    /// `args` of an `openclaw` agent go between the command and `--channel`, so
    /// `openclaw message send` is written as a program plus two arguments.
    #[tokio::test]
    async fn openclaw_args_precede_the_channel_options() {
        let agent: crate::config::AgentConfig = toml::from_str(
            "id = 'david'\nname = 'David'\nagent_type = 'openclaw'\nmessage_template = '{key}'\n\
             [openclaw]\ncommand = 'printf'\nargs = ['[%s]']\nchannel = 'signal'\ntarget = 'uuid:u1'\n",
        )
        .expect("parse agent");
        let out = dispatch(
            &base_config(),
            &Background::new(1),
            None,
            &agent,
            &issue_assigned_delivery(),
        )
        .await
        .detail;
        assert_eq!(
            out,
            "ok: [--channel][signal][--target][uuid:u1][--message][BIL-4C3B2A19]"
        );
    }

    /// The same for an `openprx` agent that sends through a command.
    #[tokio::test]
    async fn openprx_args_precede_the_channel_options() {
        let agent: crate::config::AgentConfig = toml::from_str(
            "id = 'vano'\nname = 'Vano'\nagent_type = 'openprx'\nmessage_template = '{key}'\n\
             [openprx]\ncommand = 'sh'\nargs = ['-c', 'test \"$*\" = \"--channel signal --target t --message BIL-4C3B2A19\"', 'sh']\n\
             target = 't'\n",
        )
        .expect("parse agent");
        let out = dispatch(
            &base_config(),
            &Background::new(1),
            None,
            &agent,
            &issue_assigned_delivery(),
        )
        .await
        .detail;
        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn the_four_forwarding_agent_types_start_without_extra_flags() {
        let config = base_config();
        let payload = json!({"event":"issue.created"});

        let openclaw: crate::config::AgentConfig =
            toml::from_str("id='o1'\nname='OpenClaw'\nagent_type='openclaw'").unwrap();
        let openprx: crate::config::AgentConfig =
            toml::from_str("id='o2'\nname='OpenPRX'\nagent_type='openprx'").unwrap();
        let webhook: crate::config::AgentConfig =
            toml::from_str("id='o3'\nname='Webhook'\nagent_type='webhook'").unwrap();
        let custom: crate::config::AgentConfig = toml::from_str("id='o4'\nname='Custom'\nagent_type='custom'").unwrap();

        assert!(
            dispatch(&config, &Background::new(1), None, &openclaw, &payload)
                .await
                .detail
                .contains("missing openclaw config")
        );
        assert!(
            dispatch(&config, &Background::new(1), None, &openprx, &payload)
                .await
                .detail
                .contains("missing openprx config")
        );
        assert!(
            dispatch(&config, &Background::new(1), None, &webhook, &payload)
                .await
                .detail
                .contains("missing webhook config")
        );
        assert!(
            dispatch(&config, &Background::new(1), None, &custom, &payload)
                .await
                .detail
                .contains("missing custom config")
        );
    }
}
