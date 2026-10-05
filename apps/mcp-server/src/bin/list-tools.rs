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

/// Any other write failure (a full device, an I/O error) means the listing is incomplete, so
/// it exits non-zero. The reason goes to stderr if stderr can still be written; it is dropped
/// otherwise, because `eprintln!` would panic instead.
fn main() -> ExitCode {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match write_tools(&mut out) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            let mut stderr = io::stderr().lock();
            let reported = writeln!(stderr, "Failed to write tool list: {error}");
            drop(reported.and_then(|()| stderr.flush()));
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}
