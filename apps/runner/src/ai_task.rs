//! Terminal callbacks for worker AI tasks.
//!
//! The Sylvode worker marks an AI task `processing` when it delivers it (an envelope with
//! `task_id` and `ai_participant_id`) and nothing but the API's
//! `POST /api/v1/ai/callbacks/task/{task_id}/complete` and `.../fail` ends that state. With
//! `[ai_task_callback]` enabled, the agent that ran the task calls one of them when its run ends:
//! `complete` after a successful run, `fail` after any other end (the API then schedules a retry
//! while attempts remain, or marks the task failed).
//!
//! Both endpoints accept only the task's AI participant (its bot token) or a system administrator,
//! and answer HTTP 200 with the `{code, message, data}` envelope even when they refuse, so the
//! business `code` decides. Each token therefore works only for the tasks of the bot that owns it:
//! the token is chosen per task ([`Config::ai_task_callback_for`]), and a task whose bot has no
//! token is not reported at all rather than reported with another bot's token.
//!
//! Every callback names the attempt it ran (`?attempt=N`, the delivery's `attempts`), and the API
//! accepts it only while that attempt is `processing`. A callback that arrives after the worker
//! timed the attempt out and re-queued or failed the task, or after another callback ended it, is
//! refused with code 400 and logged; it never ends a later attempt. A refusal is never retried;
//! only a transport error, an HTTP 5xx or 429, or a business code of 500 or more is. A retry after
//! a request whose answer was lost is then refused the same way, so it cannot complete a failed
//! task or fail a completed one.

use crate::background::Background;
use crate::callback::MCP_SURFACE_HEADER;
use crate::config::{AgentConfig, AiTaskCallback, AiTaskCallbackTarget, Config};
use crate::dispatcher::tail_chars;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// The longest wait between two attempts, however often the delay has doubled.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// At most this many characters of an API refusal message are logged.
const MAX_LOGGED_MESSAGE_CHARS: usize = 200;
/// The `source` field of every callback body.
const SOURCE: &str = "sylvode-runner";
/// The agent named in the log for a task no agent matched.
const NO_AGENT: &str = "(none)";

/// How an agent's run of an AI task ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutcome {
    pub succeeded: bool,
    /// `success`, `failed`, `timeout`, `cancelled` or `error`.
    pub status: String,
    pub summary: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
    /// The agent's output: stdout after a success, stderr otherwise. Trimmed to its tail.
    pub output: Option<String>,
    /// Whether a failed task may be retried. `false` (the task was not run and a repeat would not
    /// run it either, such as no agent matching it) asks the API to fail it now.
    pub retryable: bool,
}

/// Which endpoint a callback goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Complete,
    Fail,
}

impl Endpoint {
    const fn path(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Fail => "fail",
        }
    }
}

/// What an answer means for the callback.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Accepted,
    /// Refused for a reason a repeat cannot change, with the API's business code when it sent one.
    Rejected {
        code: Option<i64>,
        reason: String,
    },
    /// Possibly transient.
    Retryable(String),
}

/// The API code with which the callbacks refuse a token that is not the task's participant.
const CODE_FORBIDDEN: i64 = 403;
/// The API code with which the callbacks refuse a task (or an attempt) that is no longer
/// `processing`.
const CODE_BAD_REQUEST: i64 = 400;

/// The pending terminal callback of one AI task, run by one agent.
#[derive(Debug)]
pub struct Completion {
    task_id: Uuid,
    /// The attempt the delivery belongs to (its `attempts`), sent as `?attempt=N`.
    attempt: Option<u32>,
    /// `None` for a task no agent matched.
    agent_id: Option<String>,
    agent_type: Option<String>,
    api_url: String,
    token: String,
    /// The surface header value, or `None` for a token registered for plain REST.
    surface: Option<String>,
    http_timeout: Duration,
    max_attempts: u32,
    retry_delay: Duration,
    max_output_chars: usize,
}

impl Completion {
    /// The callback `agent` owes for `payload`: `None` unless the payload is a worker AI task, the
    /// agent reports AI task outcomes and a token for the task's bot is configured (see
    /// [`Config::ai_task_callback_for`]).
    pub fn for_delivery(config: &Config, agent: &AgentConfig, payload: &Value) -> Option<Self> {
        Self::owed(config, Some(agent), payload)
    }

    /// The callback owed for a worker AI task in `payload` that no agent matches. The worker
    /// fails such a task itself once the receiver answers `no_agent`; this reports it too when a
    /// token of the task's bot is configured. `None` when the payload is not a worker AI task, the
    /// callback does not apply, or no token belongs to the task's bot.
    pub fn for_unmatched(config: &Config, payload: &Value) -> Option<Self> {
        Self::owed(config, None, payload)
    }

    fn owed(config: &Config, agent: Option<&AgentConfig>, payload: &Value) -> Option<Self> {
        let task = WorkerTask::of(payload)?;
        match config.ai_task_callback_for(agent, task.bot_id) {
            AiTaskCallback::Report(target) => Self::new(config, &task, agent, &target),
            AiTaskCallback::Off => None,
            AiTaskCallback::NoToken => {
                tracing::warn!(
                    task_id = task.task_id,
                    bot_id = task.bot_id,
                    agent = agent.map_or(NO_AGENT, |agent| agent.id.as_str()),
                    "AI task callback not sent: no token of this task's bot is configured (a token works only for \
                     the bot that owns it); add a [[ai_task_callback.bots]] entry for the bot. The Sylvode worker \
                     ends the task by its processing timeout, or at once when the delivery was declined"
                );
                None
            }
        }
    }

    fn new(
        config: &Config,
        task: &WorkerTask<'_>,
        agent: Option<&AgentConfig>,
        target: &AiTaskCallbackTarget<'_>,
    ) -> Option<Self> {
        let Ok(task_id) = Uuid::parse_str(task.task_id) else {
            tracing::warn!(
                agent = agent.map_or(NO_AGENT, |agent| agent.id.as_str()),
                "AI task callback skipped: the delivery's task_id is not a UUID"
            );
            return None;
        };
        let cfg = &config.ai_task_callback;
        Some(Self {
            task_id,
            attempt: task.attempt,
            agent_id: agent.map(|agent| agent.id.clone()),
            agent_type: agent.map(|agent| agent.agent_type.clone()),
            api_url: target.api_url.trim().trim_end_matches('/').to_string(),
            token: target.token.to_string(),
            surface: (target.surface != "rest").then(|| target.surface.to_string()),
            http_timeout: Duration::from_secs(config.runtime.http_timeout_secs),
            max_attempts: cfg.max_attempts.max(1),
            retry_delay: Duration::from_millis(cfg.retry_delay_ms),
            max_output_chars: cfg.max_output_chars,
        })
    }

    /// The agent id for the log.
    fn agent(&self) -> &str {
        self.agent_id.as_deref().unwrap_or(NO_AGENT)
    }

    /// Sends the callback for `outcome` in the background; shutdown waits for it.
    pub fn report_in_background(self, outcome: TaskOutcome, background: &Background) {
        let task_background = background.clone();
        background.spawn(async move { self.report(outcome, &task_background).await });
    }

    /// Sends the callback for `outcome`, retrying a transient failure up to `max_attempts` times
    /// in all. Once shutdown has begun no further retry is scheduled.
    pub async fn report(self, outcome: TaskOutcome, background: &Background) {
        let client = match reqwest::Client::builder().timeout(self.http_timeout).build() {
            Ok(client) => client,
            Err(e) => {
                tracing::error!(task_id = %self.task_id, error = %e, "AI task callback not sent: building the HTTP client failed");
                return;
            }
        };
        let (endpoint, body) = self.body(&outcome);
        let url = self.url(endpoint);
        let mut delay = self.retry_delay;
        for attempt in 1..=self.max_attempts {
            let answer = match self.request(&client, &url, &body).send().await {
                Ok(response) => {
                    let status = response.status();
                    match response.text().await {
                        Ok(text) => classify(status, &text),
                        Err(e) => Answer::Retryable(format!("reading the answer failed: {}", e.without_url())),
                    }
                }
                Err(e) => Answer::Retryable(format!("request failed: {}", e.without_url())),
            };
            let endpoint = endpoint.path();
            match answer {
                Answer::Accepted => {
                    tracing::info!(task_id = %self.task_id, agent = self.agent(), endpoint, attempt, "AI task callback accepted");
                    return;
                }
                Answer::Rejected { code, reason } => {
                    self.log_rejection(endpoint, attempt, code, &reason);
                    return;
                }
                Answer::Retryable(reason) if attempt < self.max_attempts && !background.is_shutting_down() => {
                    tracing::warn!(
                        task_id = %self.task_id,
                        agent = self.agent(),
                        endpoint,
                        attempt,
                        reason = %reason,
                        retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "AI task callback failed; retrying"
                    );
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = background.cancelled() => {}
                    }
                    delay = delay.saturating_mul(2).min(MAX_RETRY_DELAY);
                }
                Answer::Retryable(reason) => {
                    tracing::error!(
                        task_id = %self.task_id,
                        agent = self.agent(),
                        endpoint,
                        attempt,
                        reason = %reason,
                        "AI task callback failed; giving up, the Sylvode worker ends the task by its processing timeout"
                    );
                    return;
                }
            }
        }
    }

    /// Logs a refusal. A refused token and an attempt that already ended are explained, since
    /// both have a known cause; anything else is an error.
    fn log_rejection(&self, endpoint: &str, attempt: u32, code: Option<i64>, reason: &str) {
        match code {
            Some(CODE_FORBIDDEN) => tracing::warn!(
                task_id = %self.task_id,
                agent = self.agent(),
                endpoint,
                attempt,
                reason = %reason,
                "AI task callback refused: the token does not belong to the task's bot (a token works only for \
                 the bot that owns it); configure that bot's token in [[ai_task_callback.bots]] or \
                 [agents.ai_task_callback]. Not retried"
            ),
            Some(CODE_BAD_REQUEST) => tracing::warn!(
                task_id = %self.task_id,
                task_attempt = ?self.attempt,
                agent = self.agent(),
                endpoint,
                attempt,
                reason = %reason,
                "AI task callback refused: this attempt is no longer processing in Sylvode (it was already \
                 completed or failed, or the worker timed it out and re-queued or failed it). Not retried"
            ),
            _ => tracing::error!(
                task_id = %self.task_id,
                agent = self.agent(),
                endpoint,
                attempt,
                reason = %reason,
                "AI task callback refused by the Sylvode API; not retried"
            ),
        }
    }

    fn url(&self, endpoint: Endpoint) -> String {
        let base = format!(
            "{}/api/v1/ai/callbacks/task/{}/{}",
            self.api_url,
            self.task_id,
            endpoint.path()
        );
        match self.attempt {
            Some(attempt) => format!("{base}?attempt={attempt}"),
            None => base,
        }
    }

    /// The endpoint and JSON body for `outcome`. `complete` stores the whole body as the task's
    /// `result`; `fail` takes `error_message` and an optional `payload`.
    fn body(&self, outcome: &TaskOutcome) -> (Endpoint, Value) {
        let report = json!({
            "source": SOURCE,
            "agent_id": self.agent_id,
            "agent_type": self.agent_type,
            "status": outcome.status,
            "summary": outcome.summary,
            "exit_code": outcome.exit_code,
            "duration_ms": outcome.duration_ms.map(|ms| u64::try_from(ms).unwrap_or(u64::MAX)),
            "output": outcome.output.as_deref().map(|output| tail_chars(output, self.max_output_chars)),
        });
        if outcome.succeeded {
            (Endpoint::Complete, report)
        } else {
            (
                Endpoint::Fail,
                json!({
                    "error_message": outcome.summary,
                    "payload": report,
                    "retry": outcome.retryable,
                }),
            )
        }
    }

    fn request(&self, client: &reqwest::Client, url: &str, body: &Value) -> reqwest::RequestBuilder {
        let mut request = client.post(url).bearer_auth(&self.token).json(body);
        if let Some(surface) = &self.surface {
            request = request.header(MCP_SURFACE_HEADER, surface);
        }
        request
    }
}

/// A worker AI task delivery: one that carries `task_id` and `ai_participant_id`.
struct WorkerTask<'a> {
    task_id: &'a str,
    /// `ai_participant_id`: the bot whose token can end the task.
    bot_id: &'a str,
    /// `attempts`, when it is a positive integer.
    attempt: Option<u32>,
}

impl<'a> WorkerTask<'a> {
    fn of(payload: &'a Value) -> Option<Self> {
        Some(Self {
            task_id: non_empty_str(payload, "task_id")?,
            bot_id: non_empty_str(payload, "ai_participant_id")?,
            attempt: payload
                .get("attempts")
                .and_then(Value::as_u64)
                .and_then(|attempt| u32::try_from(attempt).ok())
                .filter(|attempt| *attempt > 0),
        })
    }
}

fn non_empty_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.is_empty())
}

/// Reads an answer of the complete or fail endpoint.
fn classify(status: reqwest::StatusCode, body: &str) -> Answer {
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Answer::Retryable(format!("HTTP {status}"));
    }
    if !status.is_success() {
        return Answer::Rejected {
            code: None,
            reason: format!("HTTP {status}"),
        };
    }
    let Some(code) = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("code").and_then(Value::as_i64).map(|code| (code, value)))
    else {
        return Answer::Rejected {
            code: None,
            reason: "the answer is not a Sylvode API envelope; check ai_task_callback.api_url".to_string(),
        };
    };
    match code {
        (0, _) => Answer::Accepted,
        (code, value) => {
            let message = value.get("message").and_then(Value::as_str).unwrap_or("no message");
            let message: String = message.chars().take(MAX_LOGGED_MESSAGE_CHARS).collect();
            let reason = format!("code {code}: {message}");
            if code >= 500 || code == 429 {
                Answer::Retryable(reason)
            } else {
                Answer::Rejected {
                    code: Some(code),
                    reason,
                }
            }
        }
    }
}

#[cfg(test)]
// `serde_json::Value` indexing returns `Null` for a missing key; it does not panic.
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use axum::{Router, body::Bytes, extract::State, http::HeaderMap, routing::post};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    const TASK_ID: &str = "6f1c1b0e-6a59-4c4b-9a43-1d2f3e4a5b6c";
    const PARTICIPANT_ID: &str = "0b7e2d4c-2f0a-4b8e-8f6c-3a1d9e5c7b20";

    /// One request the mock API received.
    #[derive(Debug)]
    struct Hit {
        path: String,
        query: Option<String>,
        headers: HeaderMap,
        body: Value,
    }

    /// A mock Sylvode API that answers each request with the next scripted `(status, body)`, the
    /// last one repeating.
    struct MockApi {
        url: String,
        hits: mpsc::UnboundedReceiver<Hit>,
        task: tokio::task::JoinHandle<()>,
    }

    struct MockState {
        hits: mpsc::UnboundedSender<Hit>,
        answers: tokio::sync::Mutex<Vec<(u16, String)>>,
    }

    async fn answer(
        State(state): State<Arc<MockState>>,
        uri: axum::http::Uri,
        headers: HeaderMap,
        raw: Bytes,
    ) -> (axum::http::StatusCode, String) {
        let body = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        let _ = state.hits.send(Hit {
            path: uri.path().to_string(),
            query: uri.query().map(ToString::to_string),
            headers,
            body,
        });
        let mut answers = state.answers.lock().await;
        let (status, body) = if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers.first().cloned().unwrap_or_else(|| (200, success()))
        };
        (
            axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK),
            body,
        )
    }

    impl MockApi {
        async fn start(answers: Vec<(u16, String)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let url = format!("http://{}", listener.local_addr().expect("address"));
            let (tx, hits) = mpsc::unbounded_channel();
            let state = Arc::new(MockState {
                hits: tx,
                answers: tokio::sync::Mutex::new(answers),
            });
            let app = Router::new()
                .route("/api/v1/ai/callbacks/task/{id}/{action}", post(answer))
                .with_state(state);
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self { url, hits, task }
        }

        async fn next(&mut self) -> Option<Hit> {
            tokio::time::timeout(Duration::from_secs(5), self.hits.recv())
                .await
                .ok()
                .flatten()
        }

        async fn none_within(&mut self, wait: Duration) -> bool {
            tokio::time::timeout(wait, self.hits.recv()).await.is_err()
        }
    }

    impl Drop for MockApi {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn success() -> String {
        r#"{"code":0,"message":"success","data":{}}"#.to_string()
    }

    fn refusal(code: i32, message: &str) -> String {
        format!(r#"{{"code":{code},"message":"{message}"}}"#)
    }

    fn config(extra: &str) -> Config {
        let content = format!(
            "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"s3cr3t-for-tests\"]\n\n{extra}"
        );
        Config::from_toml(&content, None).expect("config is valid")
    }

    fn api_config(api_url: &str, agent_extra: &str) -> Config {
        config(&format!(
            "[ai_task_callback]\nenabled = true\napi_url = \"{api_url}\"\ntoken = \"opr_global\"\n\
             max_attempts = 3\nretry_delay_ms = 10\nmax_output_chars = 5\n\n\
             [[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"custom\"\n\
             [agents.custom]\ncommand = \"/bin/echo\"\n{agent_extra}"
        ))
    }

    fn task_payload() -> Value {
        json!({"task_id": TASK_ID, "ai_participant_id": PARTICIPANT_ID, "attempts": 1})
    }

    fn outcome(succeeded: bool) -> TaskOutcome {
        TaskOutcome {
            succeeded,
            status: if succeeded { "success" } else { "timeout" }.to_string(),
            summary: if succeeded {
                "cli execution completed"
            } else {
                "cli execution timeout"
            }
            .to_string(),
            exit_code: succeeded.then_some(0),
            duration_ms: Some(42),
            output: Some("0123456789".to_string()),
            retryable: true,
        }
    }

    fn completion(config: &Config) -> Completion {
        let agent = config.agents.first().expect("agent");
        Completion::for_delivery(config, agent, &task_payload()).expect("a completion is owed")
    }

    #[tokio::test]
    async fn a_successful_run_completes_the_task_with_the_bot_token() {
        let mut api = MockApi::start(vec![(200, success())]).await;
        let config = api_config(&format!("{}/", api.url), "");
        completion(&config).report(outcome(true), &Background::new(1)).await;

        let hit = api.next().await.expect("complete request");
        assert_eq!(hit.path, format!("/api/v1/ai/callbacks/task/{TASK_ID}/complete"));
        assert_eq!(
            hit.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer opr_global")
        );
        assert!(
            hit.headers.get("x-sylvode-mcp-surface").is_none(),
            "rest sends no surface"
        );
        assert_eq!(hit.body["source"], "sylvode-runner");
        assert_eq!(hit.body["agent_id"], "fixer");
        assert_eq!(hit.body["status"], "success");
        assert_eq!(hit.body["exit_code"], 0);
        assert_eq!(hit.body["duration_ms"], 42);
        assert_eq!(hit.body["output"], "56789", "output is cut to its tail");
        assert!(api.none_within(Duration::from_millis(100)).await);
    }

    #[tokio::test]
    async fn a_failed_run_fails_the_task_with_its_error_message() {
        let mut api = MockApi::start(vec![(200, success())]).await;
        let config = api_config(
            &api.url,
            "[agents.ai_task_callback]\ntoken = \"opr_agent\"\nsurface = \"cli\"\n",
        );
        completion(&config).report(outcome(false), &Background::new(1)).await;

        let hit = api.next().await.expect("fail request");
        assert_eq!(hit.path, format!("/api/v1/ai/callbacks/task/{TASK_ID}/fail"));
        assert_eq!(
            hit.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer opr_agent"),
            "the agent token overrides the global one"
        );
        assert_eq!(
            hit.headers.get("x-sylvode-mcp-surface").and_then(|v| v.to_str().ok()),
            Some("cli")
        );
        assert_eq!(hit.body["error_message"], "cli execution timeout");
        assert_eq!(hit.body["payload"]["status"], "timeout");
        assert_eq!(hit.body["payload"]["exit_code"], Value::Null);
    }

    #[tokio::test]
    async fn transient_failures_are_retried_until_accepted() {
        let mut api = MockApi::start(vec![
            (503, String::new()),
            (200, refusal(503, "server draining")),
            (200, success()),
        ])
        .await;
        let config = api_config(&api.url, "");
        completion(&config).report(outcome(true), &Background::new(1)).await;

        for _ in 0..3 {
            let hit = api.next().await.expect("attempt");
            assert!(hit.path.ends_with("/complete"));
        }
        assert!(api.none_within(Duration::from_millis(100)).await, "stops once accepted");
    }

    #[tokio::test]
    async fn retries_stop_after_max_attempts() {
        let mut api = MockApi::start(vec![(500, String::new())]).await;
        let config = api_config(&api.url, "");
        completion(&config).report(outcome(false), &Background::new(1)).await;

        for _ in 0..3 {
            api.next().await.expect("attempt");
        }
        assert!(api.none_within(Duration::from_millis(100)).await, "max_attempts = 3");
    }

    #[tokio::test]
    async fn a_refusal_inside_a_200_is_not_retried() {
        let mut api = MockApi::start(vec![(200, refusal(400, "task is not in processing status"))]).await;
        let config = api_config(&api.url, "");
        completion(&config).report(outcome(true), &Background::new(1)).await;

        api.next().await.expect("one attempt");
        assert!(api.none_within(Duration::from_millis(100)).await, "a refusal is final");
    }

    #[tokio::test]
    async fn a_transport_error_is_retried_and_then_given_up() {
        // Nothing listens on the port of a dropped listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let config = api_config(&url, "");
        let started = std::time::Instant::now();
        completion(&config).report(outcome(true), &Background::new(1)).await;
        // Two retries: 10 ms, then 20 ms.
        assert!(
            started.elapsed() >= Duration::from_millis(30),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn shutdown_stops_further_retries() {
        let mut api = MockApi::start(vec![(500, String::new())]).await;
        let config = api_config(&api.url, "");
        let background = Background::new(1);
        background.begin_shutdown();
        completion(&config).report(outcome(true), &background).await;

        api.next().await.expect("the one attempt still goes out");
        assert!(api.none_within(Duration::from_millis(100)).await);
    }

    #[test]
    fn classifies_answers() {
        use reqwest::StatusCode;
        assert_eq!(classify(StatusCode::OK, &success()), Answer::Accepted);
        assert!(
            matches!(classify(StatusCode::OK, &refusal(403, "forbidden")), Answer::Rejected { code: Some(403), reason } if reason.contains("forbidden"))
        );
        assert!(matches!(
            classify(StatusCode::OK, &refusal(500, "x")),
            Answer::Retryable(_)
        ));
        assert!(matches!(
            classify(StatusCode::OK, &refusal(429, "x")),
            Answer::Retryable(_)
        ));
        assert!(matches!(classify(StatusCode::OK, "<html>"), Answer::Rejected { .. }));
        assert!(matches!(
            classify(StatusCode::OK, r#"{"ok":true}"#),
            Answer::Rejected { .. }
        ));
        assert!(matches!(classify(StatusCode::NOT_FOUND, ""), Answer::Rejected { .. }));
        assert!(matches!(classify(StatusCode::BAD_GATEWAY, ""), Answer::Retryable(_)));
        assert!(matches!(
            classify(StatusCode::TOO_MANY_REQUESTS, ""),
            Answer::Retryable(_)
        ));
    }

    #[test]
    fn only_worker_ai_tasks_owe_a_callback() {
        let config = api_config("http://127.0.0.1:9", "");
        let agent = config.agents.first().expect("agent");
        assert!(Completion::for_delivery(&config, agent, &task_payload()).is_some());
        assert!(Completion::for_delivery(&config, agent, &json!({"task_id": TASK_ID})).is_none());
        assert!(Completion::for_delivery(&config, agent, &json!({"event": "issue.assigned"})).is_none());
        assert!(
            Completion::for_delivery(
                &config,
                agent,
                &json!({"task_id": "../../admin", "ai_participant_id": PARTICIPANT_ID})
            )
            .is_none(),
            "a task_id that is not a UUID never reaches a URL path"
        );
    }

    #[test]
    fn disabled_safe_mode_and_opted_out_agents_owe_nothing() {
        let disabled = config(
            "[[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"custom\"\n[agents.custom]\ncommand = \"/bin/echo\"\n",
        );
        let agent = disabled.agents.first().expect("agent");
        assert!(Completion::for_delivery(&disabled, agent, &task_payload()).is_none());

        let mut safe = api_config("http://127.0.0.1:9", "");
        safe.safe_mode = true;
        let agent = safe.agents.first().expect("agent");
        assert!(Completion::for_delivery(&safe, agent, &task_payload()).is_none());

        let opted_out = api_config("http://127.0.0.1:9", "[agents.ai_task_callback]\nenabled = false\n");
        let agent = opted_out.agents.first().expect("agent");
        assert!(Completion::for_delivery(&opted_out, agent, &task_payload()).is_none());
    }

    #[tokio::test]
    async fn an_unmatched_task_is_failed_with_the_global_token() {
        let mut api = MockApi::start(vec![(200, success())]).await;
        let config = api_config(&api.url, "");
        let completion = Completion::for_unmatched(&config, &task_payload()).expect("a fail call is owed");
        completion
            .report(
                TaskOutcome {
                    succeeded: false,
                    status: "error".to_string(),
                    summary: "no agent".to_string(),
                    exit_code: None,
                    duration_ms: None,
                    output: None,
                    retryable: false,
                },
                &Background::new(1),
            )
            .await;

        let hit = api.next().await.expect("fail request");
        assert_eq!(hit.path, format!("/api/v1/ai/callbacks/task/{TASK_ID}/fail"));
        assert_eq!(
            hit.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer opr_global")
        );
        assert_eq!(hit.body["error_message"], "no agent");
        assert_eq!(hit.body["retry"], false, "a task no agent matches is not retried");
        assert_eq!(
            hit.query.as_deref(),
            Some("attempt=1"),
            "the callback names its attempt"
        );
        assert_eq!(hit.body["payload"]["agent_id"], Value::Null);
        assert_eq!(hit.body["payload"]["agent_type"], Value::Null);
    }

    const BOT_A: &str = "0b7e2d4c-2f0a-4b8e-8f6c-3a1d9e5c7b20";
    const BOT_B: &str = "5989a9a2-c7f1-4a74-ba05-cffe01e33ad4";
    const BOT_C: &str = "9d3c6f0e-1b2a-4c5d-8e7f-0a1b2c3d4e5f";

    /// A receiver that serves bot A (the owner of the global token, routed to agent `fixer`) and
    /// bot B (its own token, no agent).
    fn two_bot_config(api_url: &str) -> Config {
        config(&format!(
            "[ai_task_callback]\nenabled = true\napi_url = \"{api_url}\"\ntoken = \"opr_bot_a\"\n\
             bot_id = \"{BOT_A}\"\nmax_attempts = 3\nretry_delay_ms = 10\n\n\
             [[ai_task_callback.bots]]\nbot_id = \"{BOT_B}\"\ntoken = \"opr_bot_b\"\nsurface = \"mcp_stdio\"\n\n\
             [[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"custom\"\n\
             [agents.route]\nbot_ids = [\"{BOT_A}\"]\n[agents.custom]\ncommand = \"/bin/echo\"\n"
        ))
    }

    fn task_for(bot: &str, attempts: u64) -> Value {
        json!({"task_id": TASK_ID, "ai_participant_id": bot, "attempts": attempts})
    }

    fn no_agent() -> TaskOutcome {
        TaskOutcome {
            succeeded: false,
            status: "error".to_string(),
            summary: "no agent".to_string(),
            exit_code: None,
            duration_ms: None,
            output: None,
            retryable: false,
        }
    }

    /// The bug the live test found: bot B's unmatched task was failed with bot A's global token and
    /// refused (403). It is now failed with bot B's own token.
    #[tokio::test]
    async fn an_unmatched_task_is_failed_with_the_token_of_its_own_bot() {
        let mut api = MockApi::start(vec![(200, success())]).await;
        let config = two_bot_config(&api.url);
        Completion::for_unmatched(&config, &task_for(BOT_B, 2))
            .expect("bot B has a token")
            .report(no_agent(), &Background::new(1))
            .await;

        let hit = api.next().await.expect("fail request");
        assert_eq!(hit.path, format!("/api/v1/ai/callbacks/task/{TASK_ID}/fail"));
        assert_eq!(hit.query.as_deref(), Some("attempt=2"));
        assert_eq!(
            hit.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer opr_bot_b")
        );
        assert_eq!(
            hit.headers.get("x-sylvode-mcp-surface").and_then(|v| v.to_str().ok()),
            Some("mcp_stdio"),
            "the bot entry's surface"
        );
    }

    /// A bot without a token of its own is not reported with another bot's token: no request at
    /// all, so no 403 either.
    #[tokio::test]
    async fn a_bot_without_a_token_is_not_reported_with_another_bots_token() {
        let mut api = MockApi::start(vec![(200, success())]).await;
        let config = two_bot_config(&api.url);
        assert!(Completion::for_unmatched(&config, &task_for(BOT_C, 1)).is_none());
        let agent = config.agents.first().expect("agent");
        assert!(
            Completion::for_delivery(&config, agent, &task_for(BOT_C, 1)).is_none(),
            "an agent without its own token does not lend bot A's token to bot C"
        );
        assert!(api.none_within(Duration::from_millis(100)).await);

        // Bot A's own tasks still use the global token.
        let completion =
            Completion::for_delivery(&config, agent, &task_for(BOT_A, 1)).expect("bot A owns the global token");
        completion.report(outcome(true), &Background::new(1)).await;
        let hit = api.next().await.expect("complete request");
        assert_eq!(
            hit.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer opr_bot_a")
        );
    }

    #[test]
    fn token_selection_prefers_the_agent_then_the_bot_entry_then_the_global_token() {
        use crate::config::AiTaskCallback;
        let token = |callback: AiTaskCallback<'_>| match callback {
            AiTaskCallback::Report(target) => Some(target.token.to_string()),
            AiTaskCallback::Off | AiTaskCallback::NoToken => None,
        };
        let config = two_bot_config("http://127.0.0.1:9");
        let agent = config.agents.first().expect("agent");
        assert_eq!(
            token(config.ai_task_callback_for(None, BOT_B)).as_deref(),
            Some("opr_bot_b")
        );
        assert_eq!(
            token(config.ai_task_callback_for(Some(agent), BOT_B)).as_deref(),
            Some("opr_bot_b")
        );
        assert_eq!(
            token(config.ai_task_callback_for(None, &BOT_A.to_uppercase())).as_deref(),
            Some("opr_bot_a")
        );
        assert_eq!(config.ai_task_callback_for(None, BOT_C), AiTaskCallback::NoToken);
        assert_eq!(config.ai_task_callback_for(None, "not-a-uuid"), AiTaskCallback::NoToken);

        let with_agent_token = api_config(
            "http://127.0.0.1:9",
            "[agents.ai_task_callback]\ntoken = \"opr_agent\"\n",
        );
        let agent = with_agent_token.agents.first().expect("agent");
        assert_eq!(
            token(with_agent_token.ai_task_callback_for(Some(agent), BOT_B)).as_deref(),
            Some("opr_agent")
        );
        // Without `bot_id` the global token keeps covering every bot, as before.
        assert_eq!(
            token(with_agent_token.ai_task_callback_for(None, BOT_C)).as_deref(),
            Some("opr_global")
        );

        let mut safe = two_bot_config("http://127.0.0.1:9");
        safe.safe_mode = true;
        assert_eq!(safe.ai_task_callback_for(None, BOT_B), AiTaskCallback::Off);
        assert_eq!(safe.ai_task_callback_for(None, BOT_B).answer(), "off");
        assert_eq!(config.ai_task_callback_for(None, BOT_C).answer(), "no_token");
        assert_eq!(config.ai_task_callback_for(None, BOT_B).answer(), "report");
    }

    /// A 403 (the token is not the task's participant) is logged and not retried.
    #[tokio::test]
    async fn a_refused_token_is_not_retried() {
        let mut api = MockApi::start(vec![(
            200,
            refusal(403, "task can only be updated by assigned ai participant"),
        )])
        .await;
        let config = api_config(&api.url, "");
        Completion::for_unmatched(&config, &task_payload())
            .expect("the global token is used without bot_id")
            .report(no_agent(), &Background::new(1))
            .await;
        api.next().await.expect("one attempt");
        assert!(api.none_within(Duration::from_millis(100)).await, "a 403 is final");
    }

    /// A delivery without a usable `attempts` sends no attempt, so the API applies its old rules.
    #[tokio::test]
    async fn a_delivery_without_attempts_names_no_attempt() {
        let mut api = MockApi::start(vec![(200, success())]).await;
        let config = api_config(&api.url, "");
        let agent = config.agents.first().expect("agent");
        for attempts in [json!(null), json!(0), json!("2")] {
            let payload = json!({"task_id": TASK_ID, "ai_participant_id": PARTICIPANT_ID, "attempts": attempts});
            Completion::for_delivery(&config, agent, &payload)
                .expect("a completion is owed")
                .report(outcome(true), &Background::new(1))
                .await;
            let hit = api.next().await.expect("complete request");
            assert_eq!(hit.query, None, "{attempts}");
        }
    }

    #[test]
    fn an_unmatched_task_needs_a_worker_task_and_a_global_token() {
        let enabled = api_config("http://127.0.0.1:9", "");
        assert!(Completion::for_unmatched(&enabled, &json!({"task_id": TASK_ID})).is_none());

        let agent_token_only = config(
            "[ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:9\"\n\n\
             [[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"custom\"\n\
             [agents.custom]\ncommand = \"/bin/echo\"\n[agents.ai_task_callback]\ntoken = \"opr_agent\"\n",
        );
        assert!(
            Completion::for_unmatched(&agent_token_only, &task_payload()).is_none(),
            "an agent's own token is not used for a task no agent matched"
        );

        let mut safe = api_config("http://127.0.0.1:9", "");
        safe.safe_mode = true;
        assert!(Completion::for_unmatched(&safe, &task_payload()).is_none());

        let disabled = config("");
        assert!(Completion::for_unmatched(&disabled, &task_payload()).is_none());
    }

    #[test]
    fn webhook_agents_do_not_report_unless_asked() {
        let forwarding = |extra: &str| {
            config(&format!(
                "[ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:9\"\ntoken = \"opr_global\"\n\n\
                 [[agents]]\nid = \"relay\"\nname = \"Relay\"\nagent_type = \"webhook\"\n\
                 [agents.webhook]\nurl = \"http://127.0.0.1:9/next\"\n{extra}"
            ))
        };
        let default = forwarding("");
        assert!(Completion::for_delivery(&default, &default.agents[0], &task_payload()).is_none());
        let asked = forwarding("[agents.ai_task_callback]\nenabled = true\n");
        assert!(Completion::for_delivery(&asked, &asked.agents[0], &task_payload()).is_some());
    }
}
