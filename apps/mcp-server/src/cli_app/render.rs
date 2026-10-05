//! The `sylvode.cli.v1` success/failure envelope (`cli-surface-v1.md` "命名与格式") and its
//! two renderings.
//!
//! `--format json` is the stable machine contract; `--format table` is a human display that
//! may evolve.

use super::error::CliError;
use serde_json::{Value, json};
use std::io::Write;

pub const SCHEMA_VERSION: &str = "sylvode.cli.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Json,
    Table,
}

/// Exit status of a command that succeeded but whose output could not be written to stdout.
///
/// Deliberately outside `error-mapping-v1.md`'s business exit codes: none of them describes a
/// local output device failing after the API already answered, and reusing one would tell a
/// script something false (`2` says nothing was sent, `9` says a retry is safe). `1` is the
/// conventional generic failure, so the caller sees a non-zero status instead of a success it
/// never received the result of.
pub const UNDELIVERED_OUTPUT_EXIT: i32 = 1;

/// Renders one command outcome to `stdout`/`stderr` and returns the process exit code.
///
/// `--format json` writes the whole envelope to stdout on both success and failure — "JSON
/// 成功/失败均只写 stdout" — so a scripted caller reads one place for either. `--format table`
/// is a human display: success renders `data` as a table on stdout, failure renders a short
/// human line on stderr and nothing on stdout, matching this binary's `mcp-server` sibling
/// CLI's existing convention for a failed call.
///
/// Nothing here panics on an unwritable stream. A failure keeps its own exit code whether or
/// not its report could be written; a success whose output could not be written exits
/// [`UNDELIVERED_OUTPUT_EXIT`], with a best-effort line on stderr.
pub fn render(format: OutputFormat, command: &str, outcome: Result<Value, CliError>, request_id: &str) -> i32 {
    let mut stdout = std::io::stdout().lock();
    match (format, outcome) {
        (OutputFormat::Json, Ok(data)) => {
            let written = writeln!(stdout, "{}", success_envelope(command, &data, request_id));
            delivered(written.and_then(|()| stdout.flush()), 0)
        }
        (OutputFormat::Json, Err(error)) => {
            let written = writeln!(stdout, "{}", failure_envelope(command, &error, request_id));
            drop(written.and_then(|()| stdout.flush()));
            error.exit
        }
        (OutputFormat::Table, Ok(data)) => {
            let written = write_table(&mut stdout, &data);
            delivered(written.and_then(|()| stdout.flush()), 0)
        }
        (OutputFormat::Table, Err(error)) => {
            let mut stderr = std::io::stderr().lock();
            let written = writeln!(stderr, "Error [{}]: {}", error.code, error.message);
            drop(written.and_then(|()| stderr.flush()));
            error.exit
        }
    }
}

/// `exit` when the output was written; otherwise [`UNDELIVERED_OUTPUT_EXIT`], after telling
/// stderr why if stderr can still be written.
fn delivered(written: std::io::Result<()>, exit: i32) -> i32 {
    match written {
        Ok(()) => exit,
        Err(error) => {
            let mut stderr = std::io::stderr().lock();
            let reported = writeln!(stderr, "Error: failed to write the result to stdout: {error}");
            drop(reported.and_then(|()| stderr.flush()));
            UNDELIVERED_OUTPUT_EXIT
        }
    }
}

fn success_envelope(command: &str, data: &Value, request_id: &str) -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "command": command,
        "data": data,
        "request_id": request_id,
    })
    .to_string()
}

fn failure_envelope(command: &str, error: &CliError, request_id: &str) -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": false,
        "command": command,
        "error": {
            "code": error.code,
            "message": error.message,
            "recoverable": error.recoverable,
            "details": error.details,
        },
        "request_id": request_id,
    })
    .to_string()
}

fn fmt_val(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn write_table(out: &mut impl Write, value: &Value) -> std::io::Result<()> {
    match value {
        Value::Object(obj) if obj.contains_key("items") && obj.get("items").is_some_and(Value::is_array) => {
            write_table(out, obj.get("items").unwrap_or(&Value::Null))?;
        }
        Value::Array(items) if !items.is_empty() => {
            if let Some(Value::Object(first)) = items.first() {
                let keys: Vec<String> = first.keys().cloned().collect();
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        writeln!(out, "---")?;
                    }
                    if let Value::Object(obj) = item {
                        let max_key = keys.iter().map(String::len).max().unwrap_or(0);
                        for key in &keys {
                            writeln!(
                                out,
                                "{key:<max_key$}  {}",
                                fmt_val(obj.get(key).unwrap_or(&Value::Null))
                            )?;
                        }
                    }
                }
            } else {
                for item in items {
                    writeln!(out, "{}", fmt_val(item))?;
                }
            }
        }
        Value::Array(_) => writeln!(out, "(empty)")?,
        Value::Object(obj) => {
            let max_key = obj.keys().map(String::len).max().unwrap_or(0);
            for (key, val) in obj {
                writeln!(out, "{key:<max_key$}  {}", fmt_val(val))?;
            }
        }
        _ => writeln!(out, "{}", fmt_val(value))?,
    }
    Ok(())
}
