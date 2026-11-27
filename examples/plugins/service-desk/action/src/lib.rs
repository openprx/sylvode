//! Sample `action` plugin of a service desk: assigns a request and closes it.
//!
//! Actions: `assign_request` with `{"record_id", "assignee"}` and `close_request` with
//! `{"record_id"}`. Each updates the record through `records.update`, so the update runs as the
//! invoking user under the user's own form and field rights. A refused host call comes back as an
//! action error that carries the refusal code, so the caller sees why.

wit_bindgen::generate!({
    path: "../../../../wit/sv-plugin.wit",
    world: "action",
});

use serde_json::{Value, json};
use sv::plugin::types::CallError;
use sv::plugin::{log, records};

struct Plugin;

/// The refusal of a host call as an action error code: lowercase `[a-z0-9_]`, at most 64 characters.
fn code_of(error: &CallError) -> String {
    let raw = match error {
        CallError::Denied(code) | CallError::Invalid(code) | CallError::Conflict(code) | CallError::Limit(code) => {
            code.as_str()
        }
        CallError::NotFound => "not_found",
        CallError::Unavailable => "unavailable",
    };
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

fn fail(code: &str) -> ActionError {
    ActionError { code: code.to_string() }
}

fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str, ActionError> {
    input.get(key).and_then(Value::as_str).ok_or_else(|| fail("bad_input"))
}

fn update(record_id: &str, values: &Value) -> Result<(), ActionError> {
    records::update(record_id, &values.to_string(), None)
        .map(|_| ())
        .map_err(|error| fail(&code_of(&error)))
}

impl Guest for Plugin {
    fn run(action: String, input: String) -> Result<String, ActionError> {
        let input: Value = serde_json::from_str(&input).map_err(|_| fail("bad_input"))?;
        let record_id = text(&input, "record_id")?;
        match action.as_str() {
            "assign_request" => {
                let assignee = text(&input, "assignee")?;
                update(record_id, &json!({"assignee": assignee, "status": "assigned"}))?;
                log::emit(log::LogLevel::Info, "assigned");
                Ok(json!({"record_id": record_id, "status": "assigned"}).to_string())
            }
            "close_request" => {
                update(record_id, &json!({"status": "closed"}))?;
                log::emit(log::LogLevel::Info, "closed");
                Ok(json!({"record_id": record_id, "status": "closed"}).to_string())
            }
            _ => Err(fail("unknown_action")),
        }
    }
}

export!(Plugin);
