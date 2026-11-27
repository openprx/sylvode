//! The store data of one invocation and the host side of the imported interfaces.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use wasmtime::ResourceLimiter;

use super::bindings::action::sv::plugin::{catalogue as wit_catalogue, clock, ctx, log, records, types};
use super::error::HostError;
use super::limits::{CALL_BODY_MAX, LOG_LINES_MAX, Limits};
use crate::catalogue::RefusalCode;
use crate::extensions::dispatch::{self, CallEnv, CallError};

/// Table elements a guest may hold.
const TABLE_ELEMENTS: usize = 65_536;

/// Memory limits of one invocation. Growth past the world's limit ends the invocation with
/// `plugin_memory_limit` at once, so a guest cannot swallow the failure and carry on.
pub struct Limiter {
    max_memory: usize,
}

impl ResourceLimiter for Limiter {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        if desired > self.max_memory {
            Err(wasmtime::Error::new(HostError::new(RefusalCode::PluginMemoryLimit)))
        } else {
            Ok(true)
        }
    }

    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= TABLE_ELEMENTS)
    }

    fn instances(&self) -> usize {
        16
    }

    fn tables(&self) -> usize {
        16
    }

    fn memories(&self) -> usize {
        4
    }
}

/// What `ctx.current` tells the guest.
#[derive(Clone, Debug)]
pub struct InvocationInfo {
    pub actor: String,
    pub workspace: String,
    pub project: Option<String>,
    pub installation: String,
    pub generation: u32,
    pub action: String,
    pub request_id: String,
}

/// The store data.
pub struct HostState {
    limiter: Limiter,
    /// `None` for a `field-rule`, which has no host calls.
    env: Option<Arc<CallEnv>>,
    info: InvocationInfo,
    deadline: Instant,
    max_host_calls: u32,
    pub host_calls: u32,
    pub log_lines: u32,
    pub denials: u32,
    pub last_denial: Option<String>,
}

impl HostState {
    #[must_use]
    pub fn new(limits: &Limits, env: Option<Arc<CallEnv>>, info: InvocationInfo, deadline: Instant) -> Self {
        Self {
            limiter: Limiter {
                max_memory: limits.memory,
            },
            env,
            info,
            deadline,
            max_host_calls: limits.host_calls,
            host_calls: 0,
            log_lines: 0,
            denials: 0,
            last_denial: None,
        }
    }

    pub fn limiter(&mut self) -> &mut dyn ResourceLimiter {
        &mut self.limiter
    }

    /// One dispatching host call: counted, bounded by the time left, run through the dispatcher.
    async fn dispatch(&mut self, op: &str, request: &str) -> wasmtime::Result<Result<String, types::CallError>> {
        self.host_calls += 1;
        if self.host_calls > self.max_host_calls {
            return Err(wasmtime::Error::new(HostError::new(RefusalCode::PluginHostCallLimit)));
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(wasmtime::Error::new(HostError::new(
                RefusalCode::PluginDeadlineExceeded,
            )));
        }
        let Some(env) = self.env.as_deref() else {
            return Ok(Err(types::CallError::Unavailable));
        };
        let outcome = dispatch::call(env, self.host_calls, op, request, remaining).await;
        Ok(match outcome {
            Ok(data) => match serde_json::to_string(&data) {
                Ok(text) if text.len() <= CALL_BODY_MAX => Ok(text),
                _ => Err(types::CallError::Limit("payload_too_large".to_string())),
            },
            Err(error) => {
                if matches!(error, CallError::Denied(_)) {
                    self.denials += 1;
                    self.last_denial = Some(error.code().to_string());
                }
                Err(to_wit(error))
            }
        })
    }
}

fn to_wit(error: CallError) -> types::CallError {
    match error {
        CallError::Denied(code) => types::CallError::Denied(code),
        CallError::NotFound => types::CallError::NotFound,
        CallError::Invalid(code) => types::CallError::Invalid(code),
        CallError::Conflict(code) => types::CallError::Conflict(code),
        CallError::Limit(code) => types::CallError::Limit(code),
        CallError::Unavailable => types::CallError::Unavailable,
    }
}

fn parse_json(text: &str) -> Result<Value, types::CallError> {
    serde_json::from_str(text).map_err(|_| types::CallError::Invalid("bad_request".to_string()))
}

impl types::Host for HostState {}

impl ctx::Host for HostState {
    async fn current(&mut self) -> wasmtime::Result<ctx::Invocation> {
        Ok(ctx::Invocation {
            principal_kind: types::PrincipalKind::User,
            actor: self.info.actor.clone(),
            workspace: self.info.workspace.clone(),
            project: self.info.project.clone(),
            installation: self.info.installation.clone(),
            generation: self.info.generation,
            action: self.info.action.clone(),
            locale: "en".to_string(),
            request_id: self.info.request_id.clone(),
        })
    }
}

impl records::Host for HostState {
    async fn get(&mut self, record_id: String) -> wasmtime::Result<Result<String, types::CallError>> {
        self.dispatch(
            "forms.records.read",
            &json!({"path": {"record_id": record_id}}).to_string(),
        )
        .await
    }

    async fn query(
        &mut self,
        form_id: String,
        query: Vec<(String, String)>,
    ) -> wasmtime::Result<Result<String, types::CallError>> {
        let query: serde_json::Map<String, Value> = query.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        self.dispatch(
            "forms.records.read/list",
            &json!({"path": {"form_id": form_id}, "query": query}).to_string(),
        )
        .await
    }

    async fn create(
        &mut self,
        form_id: String,
        values: String,
        title: Option<String>,
    ) -> wasmtime::Result<Result<String, types::CallError>> {
        let values = match parse_json(&values) {
            Ok(values) => values,
            Err(error) => return Ok(Err(error)),
        };
        self.dispatch(
            "forms.records.create",
            &json!({"path": {"form_id": form_id}, "body": {"values": values, "title": title}}).to_string(),
        )
        .await
    }

    async fn update(
        &mut self,
        record_id: String,
        values: String,
        expected_updated_at: Option<String>,
    ) -> wasmtime::Result<Result<String, types::CallError>> {
        let values = match parse_json(&values) {
            Ok(values) => values,
            Err(error) => return Ok(Err(error)),
        };
        self.dispatch(
            "forms.records.update",
            &json!({
                "path": {"record_id": record_id},
                "body": {"values": values, "expected_updated_at": expected_updated_at}
            })
            .to_string(),
        )
        .await
    }
}

impl wit_catalogue::Host for HostState {
    async fn call(&mut self, op: String, request: String) -> wasmtime::Result<Result<String, types::CallError>> {
        // The dispatcher holds every size and shape check of the guest's text.
        self.dispatch(&op, &request).await
    }
}

impl log::Host for HostState {
    /// Counts the line and drops it: guest text is never written (contract §5, SF-10).
    async fn emit(&mut self, _level: log::LogLevel, _message: String) -> wasmtime::Result<()> {
        self.log_lines = (self.log_lines + 1).min(LOG_LINES_MAX);
        Ok(())
    }
}

impl clock::Host for HostState {
    async fn now_ms(&mut self) -> wasmtime::Result<u64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX));
        Ok(now)
    }
}
