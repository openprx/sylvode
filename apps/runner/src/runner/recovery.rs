//! Restart recovery (design 7.8): the state of every task is rebuilt from Sylvode, never from
//! memory. For each bot the runner serves, `GET /ai/tasks/active` lists its open tasks; each one
//! that belongs to a tmux project of this runner is adopted, failed or queued again, by
//! [`plan_task`]. Tasks of other channels, other keys the runner cannot serve, and other bots are
//! left alone (the lease sweep handles those).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::checks::{self, BotCreds, Channel, Context, Verdict};
use super::session::{Adopt, Probe, TmuxTask};
use super::sv_client::{SvClient, TASK_PROTOCOL};
use super::tmux::SessionKey;
use crate::AppState;

/// What to do with one active task after a restart.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// The task was pushed and its session lives: supervise it again.
    Adopt { acknowledged: bool },
    /// The task was blocked on a dialog and its session lives: resume as blocked.
    Blocked,
    /// The task was pushed (or blocked) and the session is gone: fail it, retryable.
    FailGone,
    /// Never pushed: queue it again.
    Requeue,
    /// A phase this runner does not know: leave the task alone.
    Skip,
}

/// What Sylvode says about the task and what the runner finds in tmux.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools, reason = "independent facts about one task")]
pub struct Facts<'a> {
    pub phase: Option<&'a str>,
    pub status: &'a str,
    pub acknowledged: bool,
    pub session_alive: bool,
    /// The scrollback of the pane contains `id=<task_id>`.
    pub scrollback_hit: bool,
    /// The runner's own `session.json` says this task (at this attempt) was typed in.
    pub recorded_pushed: bool,
}

/// Design 7.8, step 2, with one rule above the others: a task that may have been typed into a
/// session is never typed again. It counts as pushed when Sylvode shows it acknowledged (whatever
/// the phase says: the phase is reported fire-and-forget and may be stale), when the runner's own
/// record of the session says so, or when its message is in the scrollback.
pub fn plan_task(facts: &Facts<'_>) -> Step {
    let acknowledged = facts.acknowledged || facts.status == "acknowledged";
    let pushed = acknowledged
        || facts.recorded_pushed
        || facts.phase == Some("pushed")
        || (facts.session_alive && facts.scrollback_hit);
    match facts.phase {
        Some("blocked") if facts.session_alive => Step::Blocked,
        Some("blocked") => Step::FailGone,
        Some("queued" | "pushed") | None if pushed => {
            if facts.session_alive {
                Step::Adopt { acknowledged }
            } else {
                Step::FailGone
            }
        }
        // Never pushed, as far as anything shows: it is safe to queue it again.
        Some("queued") | None => Step::Requeue,
        Some(_) => Step::Skip,
    }
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str()
}

/// Recovers the tasks of every bot of the `runner` agents. Failures are logged and leave the
/// tasks to the lease sweep.
pub async fn recover_all(state: Arc<AppState>) {
    let Some(sessions) = state.sessions.clone() else { return };
    let Ok(http) = &state.runner.http else { return };
    for target in crate::handshake::targets(&state.config) {
        let client = SvClient::new(http.clone(), &target.api_url, &target.token, &target.surface);
        let bot = BotCreds {
            bot_id: target.bot_id.clone(),
            api_url: target.api_url.clone(),
            token: target.token.clone(),
        };
        let timeout = Duration::from_secs(state.config.runner.delivery_check_timeout_secs);
        let views = match client.active_tasks(timeout).await {
            Ok(views) => views,
            Err(e) => {
                tracing::warn!(
                    "restart recovery: the active tasks of bot {} are not available: {e}",
                    target.bot_id
                );
                continue;
            }
        };
        recover_bot(&state, &sessions, &client, &bot, views, timeout).await;
    }
}

#[derive(Default)]
struct PerKey {
    adopt: Option<Adopt>,
    requeue: Vec<TmuxTask>,
}

async fn recover_bot(
    state: &Arc<AppState>,
    sessions: &Arc<super::session::SessionManager>,
    client: &SvClient,
    bot: &BotCreds,
    views: Vec<Value>,
    timeout: Duration,
) {
    let mut keys: Vec<(String, PerKey)> = Vec::new();
    for view in views {
        let (Some(task_id), Some(key_text)) = (str_at(&view, &["task_id"]), str_at(&view, &["project", "key"])) else {
            continue;
        };
        if str_at(&view, &["execution", "channel"]) != Some("tmux") || !checks::valid_project_key(key_text) {
            continue;
        }
        let Ok(key) = SessionKey::new(key_text) else { continue };
        let attempt = view.get("attempt").and_then(Value::as_i64).unwrap_or(0);
        let probe: Probe = sessions.probe(&key, task_id, attempt).await;
        let step = plan_task(&Facts {
            phase: view.get("runner_phase").and_then(Value::as_str),
            status: str_at(&view, &["status"]).unwrap_or_default(),
            acknowledged: view.get("acknowledged_at").is_some_and(|v| !v.is_null()),
            session_alive: probe.alive,
            scrollback_hit: probe.scrollback_hit,
            recorded_pushed: probe.recorded_pushed,
        });
        if step == Step::FailGone {
            let reason = format!("runner restarted and session {key_text} was gone");
            if let Err(e) = client.fail(task_id, attempt, &reason, true, timeout).await {
                tracing::warn!("restart recovery: could not fail a task: {e}");
            }
            continue;
        }
        if step == Step::Skip {
            continue;
        }
        let Some(task) = rebuild_task(state, client, bot, &view).await else {
            continue;
        };
        let at = keys.iter().position(|(k, _)| k == key_text).unwrap_or_else(|| {
            keys.push((key_text.to_string(), PerKey::default()));
            keys.len() - 1
        });
        let Some((_, entry)) = keys.get_mut(at) else { continue };
        match step {
            Step::Adopt { acknowledged } if entry.adopt.is_none() => {
                entry.adopt = Some(Adopt {
                    task: Box::new(task),
                    acknowledged,
                    blocked: false,
                });
            }
            Step::Blocked if entry.adopt.is_none() => {
                entry.adopt = Some(Adopt {
                    task: Box::new(task),
                    acknowledged: view.get("acknowledged_at").is_some_and(|v| !v.is_null()),
                    blocked: true,
                });
            }
            // A session has one open task: a second one that may have been typed in is failed,
            // never typed again.
            Step::Adopt { .. } | Step::Blocked => {
                let reason = format!("runner restarted with two open tasks in session {key_text}");
                if let Err(e) = client.fail(task_id, attempt, &reason, false, timeout).await {
                    tracing::warn!("restart recovery: could not fail a task: {e}");
                }
            }
            _ => entry.requeue.push(task),
        }
    }
    let mut by_key: HashMap<String, PerKey> = keys.into_iter().collect();
    for (key_text, entry) in by_key.drain() {
        if let Ok(key) = SessionKey::new(&key_text) {
            tracing::info!(session = %key_text, adopted = entry.adopt.is_some(), requeued = entry.requeue.len(), "restart recovery");
            sessions.recover(key, entry.adopt, entry.requeue).await;
        }
    }
}

/// Runs the delivery checks on a view, as if Sylvode had delivered it again, to get a task that
/// has every field the session worker needs (and a directory that is still allowed).
async fn rebuild_task(state: &Arc<AppState>, client: &SvClient, bot: &BotCreds, view: &Value) -> Option<TmuxTask> {
    let registry = state.runner.registry.as_ref().ok()?;
    let payload = json!({
        "protocol": TASK_PROTOCOL,
        "task_id": str_at(view, &["task_id"])?,
        "project_id": str_at(view, &["project", "id"])?,
        "project_key": str_at(view, &["project", "key"])?,
        "ai_participant_id": bot.bot_id,
        "attempts": view.get("attempt").and_then(Value::as_i64)?,
        "max_attempts": view.get("max_attempts").and_then(Value::as_i64).unwrap_or(1),
    });
    let verdict = checks::check_delivery(
        Context {
            config: &state.config,
            roots: &state.runner.roots,
            registry,
            protocol_mismatch: None,
            client: client.clone(),
            bot: bot.clone(),
        },
        &payload,
    )
    .await;
    match verdict {
        Verdict::Run(accepted) if accepted.channel == Channel::Tmux => Some(TmuxTask::from_accepted(&accepted)),
        other => {
            tracing::info!("restart recovery leaves a task alone: {other:?}");
            None
        }
    }
}
