//! An in-process mock of the Sylvode task protocol API: the protocol, the runner report, the task
//! view, progress and fail. It records every request and answers from a script the test edits.

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// One request the mock received.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    /// Path and query.
    pub target: String,
    pub authorization: Option<String>,
    pub surface: Option<String>,
    pub body: Value,
}

impl Req {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }
}

/// What the mock answers.
pub struct Script {
    pub protocol: Value,
    pub protocol_status: u16,
    pub report_status: u16,
    /// Task views by task id: answered in order, the last one repeats.
    pub views: HashMap<String, Vec<(u16, Value)>>,
    pub view_delay: Duration,
    /// The answer of `GET /ai/tasks/active`.
    pub active: Vec<Value>,
    pub progress_status: u16,
    pub fail_status: u16,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            protocol: json!({
                "task_protocol": 1,
                "api_version": "0.5.0",
                "features": ["ack", "lease", "reply_routing", "runner_phase", "runner_report"],
                "runner_report_interval_secs": 600,
            }),
            protocol_status: 200,
            report_status: 200,
            views: HashMap::new(),
            view_delay: Duration::ZERO,
            active: Vec::new(),
            progress_status: 200,
            fail_status: 200,
        }
    }
}

struct Inner {
    script: Mutex<Script>,
    log: Mutex<Vec<Req>>,
}

pub struct MockSv {
    pub addr: SocketAddr,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

fn envelope(status: u16, data: &Value) -> Response {
    let body = if status < 300 {
        json!({"code": 0, "message": "success", "data": data})
    } else {
        json!({"code": status, "message": "refused"})
    };
    (
        StatusCode::from_u16(status).expect("status"),
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn handle(
    State(inner): State<Arc<Inner>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let request = Req {
        method: method.to_string(),
        target: uri.to_string(),
        authorization: header("authorization"),
        surface: header("x-sylvode-mcp-surface"),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    };
    let path = request.path().to_string();
    inner.log.lock().push(request);
    let (delay, answer) = {
        let mut script = inner.script.lock();
        let delay = script.view_delay;
        let answer: (u16, Value) = if path == "/api/v1/ai/protocol" {
            (script.protocol_status, script.protocol.clone())
        } else if path == "/api/v1/ai/runner/report" {
            (script.report_status, json!({"accepted": true}))
        } else if path.ends_with("/progress") {
            (script.progress_status, json!({"coalesced": false}))
        } else if path.ends_with("/fail") {
            (script.fail_status, json!({"will_retry": false}))
        } else if path == "/api/v1/ai/tasks/active" {
            (200, Value::Array(script.active.clone()))
        } else if let Some(id) = path.strip_prefix("/api/v1/ai/tasks/") {
            match script.views.get_mut(id) {
                Some(queue) if queue.len() > 1 => queue.remove(0),
                Some(queue) => queue.first().cloned().unwrap_or((404, Value::Null)),
                None => (404, Value::Null),
            }
        } else {
            (404, Value::Null)
        };
        (
            if path.starts_with("/api/v1/ai/tasks/") && !path.ends_with("/fail") && !path.ends_with("/progress") {
                delay
            } else {
                Duration::ZERO
            },
            answer,
        )
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    envelope(answer.0, &answer.1)
}

impl MockSv {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        let inner = Arc::new(Inner {
            script: Mutex::new(Script::default()),
            log: Mutex::new(Vec::new()),
        });
        let app = Router::new().fallback(handle).with_state(Arc::clone(&inner));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, inner, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn script<T>(&self, edit: impl FnOnce(&mut Script) -> T) -> T {
        edit(&mut self.inner.script.lock())
    }

    pub fn requests(&self) -> Vec<Req> {
        self.inner.log.lock().clone()
    }

    pub fn requests_to(&self, method: &str, path_prefix: &str) -> Vec<Req> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method && r.path().starts_with(path_prefix))
            .collect()
    }

    /// Waits until at least `count` requests match, then returns them.
    pub async fn wait_for(&self, method: &str, path_suffix: &str, count: usize, timeout: Duration) -> Vec<Req> {
        let deadline = Instant::now() + timeout;
        loop {
            let found: Vec<Req> = self
                .requests()
                .into_iter()
                .filter(|r| r.method == method && r.path().ends_with(path_suffix))
                .collect();
            if found.len() >= count || Instant::now() >= deadline {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for MockSv {
    fn drop(&mut self) {
        self.task.abort();
    }
}
