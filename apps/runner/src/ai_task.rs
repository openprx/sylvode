//! Ending a task whose forwarding failed.
//!
//! A task of the Sylvode worker is `processing` from the moment it is delivered, and only the task
//! protocol ends it (`POST /api/v1/ai/tasks/{task_id}/fail`, or `complete` by the agent through
//! the task tools). The forwarding agent types (`webhook`, `openclaw`, `openprx`, `custom`) hand
//! the delivery to another program, which owns the task from then on: a successful hand-over
//! reports nothing. This module covers the one case the receiving side cannot: the hand-over
//! itself failed, so nobody will ever end the task. The runner then fails it with
//! `source: "runner"`, using the token of the task's bot (a token works only for the tasks of the
//! bot that owns it). `retryable` is `true` for a failure a repeat may fix, `false` for a task no
//! agent will take.
//!
//! The call names the attempt (`?attempt=N`, the delivery's `attempts`), so a late report cannot
//! end a later attempt. A refusal is never retried; a timeout, a transport error or an HTTP 5xx is,
//! up to `[ai_task_callback] max_attempts` times in all.

use crate::background::Background;
use crate::config::{AiTaskCallback, Config};
use crate::runner::sv_client::{SvClient, SvError};
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

/// The longest wait between two attempts, however often the delay has doubled.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// The pending failure report of one task.
#[derive(Debug)]
pub struct ForwardFailure {
    client: SvClient,
    task_id: String,
    attempt: i64,
    max_attempts: u32,
    retry_delay: Duration,
    timeout: Duration,
}

impl ForwardFailure {
    /// The report owed for `payload`, a delivery: `None` unless it is a worker AI task with a
    /// valid task id and attempt, `[ai_task_callback]` applies, and a token of the task's bot is
    /// configured.
    pub fn for_delivery(config: &Config, http: &reqwest::Client, payload: &Value) -> Option<Self> {
        let task_id = payload.get("task_id").and_then(Value::as_str)?;
        let bot = payload.get("ai_participant_id").and_then(Value::as_str)?;
        if Uuid::parse_str(task_id).is_err() {
            tracing::warn!("AI task report skipped: the delivery's task_id is not a UUID");
            return None;
        }
        let attempt = payload.get("attempts").and_then(Value::as_i64).unwrap_or(1).max(1);
        match config.ai_task_callback_for(bot) {
            AiTaskCallback::Report(target) => {
                let cfg = &config.ai_task_callback;
                Some(Self {
                    client: SvClient::new(http.clone(), target.api_url, target.token, target.surface),
                    task_id: task_id.to_string(),
                    attempt,
                    max_attempts: cfg.max_attempts.max(1),
                    retry_delay: Duration::from_millis(cfg.retry_delay_ms),
                    timeout: Duration::from_secs(config.runtime.http_timeout_secs),
                })
            }
            AiTaskCallback::Off => None,
            AiTaskCallback::NoToken => {
                tracing::warn!(
                    task_id,
                    bot_id = bot,
                    "AI task failure not reported: no token of this task's bot is configured (a token works only \
                     for the bot that owns it); add a [[ai_task_callback.bots]] entry for the bot. The Sylvode \
                     worker ends the task by its processing timeout"
                );
                None
            }
        }
    }

    /// Reports the failure in the background; shutdown waits for it.
    pub fn report_in_background(self, reason: String, retryable: bool, background: &Background) {
        let task_background = background.clone();
        background.spawn(async move { self.report(&reason, retryable, &task_background).await });
    }

    /// Fails the task, retrying a transient failure. Once shutdown has begun no further retry is
    /// scheduled.
    pub async fn report(self, reason: &str, retryable: bool, background: &Background) {
        let mut delay = self.retry_delay;
        for attempt in 1..=self.max_attempts {
            let result = self
                .client
                .fail(&self.task_id, self.attempt, reason, retryable, self.timeout)
                .await;
            match result {
                Ok(()) => {
                    tracing::info!(task_id = %self.task_id, attempt, "AI task failure reported");
                    return;
                }
                Err(e) if e.is_transient() && attempt < self.max_attempts && !background.is_shutting_down() => {
                    tracing::warn!(
                        task_id = %self.task_id,
                        attempt,
                        error = %e,
                        retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "AI task failure report failed; retrying"
                    );
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = background.cancelled() => {}
                    }
                    delay = delay.saturating_mul(2).min(MAX_RETRY_DELAY);
                }
                Err(SvError::Status(400)) => {
                    tracing::warn!(
                        task_id = %self.task_id,
                        "AI task failure not accepted: this attempt is no longer active in Sylvode (it ended, or \
                         the worker timed it out). Not retried"
                    );
                    return;
                }
                Err(e) => {
                    tracing::error!(
                        task_id = %self.task_id,
                        attempt,
                        error = %e,
                        "AI task failure report failed; giving up, the Sylvode worker ends the task by its \
                         processing timeout"
                    );
                    return;
                }
            }
        }
    }
}
