// Standalone tool listing binary — does not require a database connection.
use mcp_server::get_all_tool_definitions;
use std::io::{self, Write};
use std::process::ExitCode;

/// Writes the tool catalogue. A closed pipe (`| head`) is a normal end of output,
/// not a failure, so it stops quietly instead of panicking inside `println!`.
fn write_tools(out: &mut impl Write) -> io::Result<()> {
    let tools = get_all_tool_definitions();
    let count = tools.len();
    writeln!(out, "Available MCP Tools ({count} total):\n")?;

    for tool in tools {
        writeln!(out, "  {}", tool.name)?;
        writeln!(out, "   {}", tool.description)?;
        let schema = serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default();
        writeln!(out, "   Schema: {schema}")?;
        writeln!(out)?;
    }
    out.flush()
}

/// Writes name, access and category of every registry tool as JSON.
fn write_json(out: &mut impl Write) -> io::Result<()> {
    let rows = mcp_server::tools::catalog::list_json();
    writeln!(out, "{}", serde_json::to_string_pretty(&rows).unwrap_or_default())?;
    out.flush()
}

/// Any other write failure (a full device, an I/O error) means the listing is incomplete, so
/// it exits non-zero. The reason goes to stderr if stderr can still be written; it is dropped
/// otherwise, because `eprintln!` would panic instead.
fn main() -> ExitCode {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut arguments = std::env::args().skip(1);
    let json_output = match (arguments.next().as_deref(), arguments.next()) {
        (None, _) => false,
        (Some("--json"), None) => true,
        _ => {
            let mut stderr = io::stderr().lock();
            drop(writeln!(stderr, "usage: list-tools [--json]"));
            return ExitCode::from(2);
        }
    };
    let written = if json_output {
        write_json(&mut out)
    } else {
        write_tools(&mut out)
    };
    match written {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            let mut stderr = io::stderr().lock();
            let reported = writeln!(stderr, "Failed to write tool list: {error}");
            drop(reported.and_then(|()| stderr.flush()));
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}
