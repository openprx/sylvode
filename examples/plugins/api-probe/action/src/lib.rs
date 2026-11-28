//! Probe `action` plugin: calls catalogue operations through the host and reports each outcome.
//!
//! It exists to test plugin exposure, grant scoping, quotas and denials against a running
//! instance; it has no business behaviour of its own.
//!
//! Actions:
//! - `call` with `{"op", "request"}`: one catalogue call; the answer carries the response data.
//! - `probe` with `{"calls": [{"op", "request", "repeat"}, ...]}`: every call in order, `repeat`
//!   times each (default 1, at most 50). A refused call does not stop the list. The answer holds
//!   one entry per call with its outcome and the size of the data, never the data itself.
//!
//! `request` is the catalogue request `{"path": {..}, "query": {..}, "body": ..}`; it may be left out
//! for an operation that needs none. A host refusal is an outcome, not an action error: the entry
//! carries the error kind and the refusal code.

wit_bindgen::generate!({
    path: "../../../../wit/sv-plugin.wit",
    world: "action",
});

use serde_json::{Value, json};
use sv::plugin::catalogue;
use sv::plugin::types::CallError;

struct Plugin;

/// The most repeats of one call in a `probe` list.
const MAX_REPEAT: u64 = 50;
/// The most entries of a `probe` list.
const MAX_CALLS: usize = 100;

fn fail(code: &str) -> ActionError {
    ActionError { code: code.to_string() }
}

/// A refusal code as `[a-z0-9_]`, at most 64 characters.
fn clean(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// The kind and code of a host refusal.
fn describe(error: &CallError) -> (&'static str, String) {
    match error {
        CallError::Denied(code) => ("denied", clean(code)),
        CallError::Invalid(code) => ("invalid", clean(code)),
        CallError::Conflict(code) => ("conflict", clean(code)),
        CallError::Limit(code) => ("limit", clean(code)),
        CallError::NotFound => ("not_found", "not_found".to_string()),
        CallError::Unavailable => ("unavailable", "unavailable".to_string()),
    }
}

/// One call: the op name and the request document, as the catalogue host takes them.
fn invoke(op: &str, request: Option<&Value>) -> Result<Value, CallError> {
    let request = request.cloned().unwrap_or_else(|| json!({}));
    let answer = catalogue::call(op, &request.to_string())?;
    Ok(serde_json::from_str(&answer).unwrap_or(Value::String(answer)))
}

fn outcome(op: &str, index: usize, result: &Result<Value, CallError>, with_data: bool) -> Value {
    match result {
        Ok(data) => {
            let mut entry = json!({"index": index, "op": op, "ok": true, "bytes": data.to_string().len()});
            if with_data {
                entry["data"] = data.clone();
            }
            entry
        }
        Err(error) => {
            let (kind, code) = describe(error);
            json!({"index": index, "op": op, "ok": false, "kind": kind, "code": code})
        }
    }
}

fn parse(input: &str) -> Result<Value, ActionError> {
    serde_json::from_str(input).map_err(|_| fail("bad_input"))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, ActionError> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| fail("bad_input"))
}

fn single(input: &Value) -> Result<String, ActionError> {
    let op = text(input, "op")?;
    let result = invoke(op, input.get("request"));
    Ok(json!({"results": [outcome(op, 0, &result, true)]}).to_string())
}

fn list(input: &Value) -> Result<String, ActionError> {
    let calls = input
        .get("calls")
        .and_then(Value::as_array)
        .ok_or_else(|| fail("bad_input"))?;
    if calls.is_empty() || calls.len() > MAX_CALLS {
        return Err(fail("bad_input"));
    }
    let mut results = Vec::new();
    for call in calls {
        let op = text(call, "op")?;
        let repeat = call
            .get("repeat")
            .map_or(Some(1), Value::as_u64)
            .ok_or_else(|| fail("bad_input"))?;
        if repeat == 0 || repeat > MAX_REPEAT {
            return Err(fail("bad_input"));
        }
        for _ in 0..repeat {
            let result = invoke(op, call.get("request"));
            results.push(outcome(op, results.len(), &result, false));
        }
    }
    Ok(json!({"results": results}).to_string())
}

impl Guest for Plugin {
    fn run(action: String, input: String) -> Result<String, ActionError> {
        let input = parse(&input)?;
        match action.as_str() {
            "call" => single(&input),
            "probe" => list(&input),
            _ => Err(fail("unknown_action")),
        }
    }
}

export!(Plugin);
