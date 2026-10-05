//! `sylvode` — the Sylvode CLI (`cli-surface-v1.md`, ADR-0020 D5).
//!
//! A new `[[bin]]` in the existing `mcp-server` package rather than a new crate or a rename of
//! the existing binary (`cli-surface-v1.md` "交付形态": "选择同 crate 新 `[[bin]]`，不 rename
//! 现有 binary、不建新 crate"). It carries two command models:
//!
//! * the Flow groups, with their own `mcp_server::cli_app` command model, config resolver,
//!   typed error and `sylvode.cli.v1` JSON renderer;
//! * the nine workspace groups (`projects` … `tools`), which are `mcp_server::cli`'s
//!   definitions and handler — the same ones `mcp-server` runs — so the two executables print
//!   the same bytes and exit with the same codes for them.
//!
//! `mcp_server::cli_app::entry::route` picks the model from the first subcommand name. It never
//! starts a server: there is no `serve` subcommand here.

use clap::{FromArgMatches, Parser};
use mcp_server::cli;
use mcp_server::cli_app::{
    self,
    command::Cli as FlowCli,
    entry::{self, Route},
};
use std::ffi::OsString;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<OsString> = std::env::args_os().collect();
    match entry::route(&args) {
        Route::Flow => run_flow(FlowCli::parse_from(&args)).await,
        Route::Business => run_business(&args).await,
        Route::Overview => {
            // Help, a usage error or a suggestion: `clap` prints it and exits here. A line that
            // nevertheless parses names a group `route` did not recognise in its position, so
            // it is handed to the parser owning that group, which reports it in its own terms.
            let mut overview = entry::overview_command();
            let matches = overview.clone().get_matches_from(&args);
            match matches.subcommand_name() {
                Some(name) if cli::BUSINESS_GROUPS.contains(&name) => run_business(&args).await,
                _ => match FlowCli::from_arg_matches(&matches) {
                    Ok(_) => run_flow(FlowCli::parse_from(&args)).await,
                    Err(error) => error.format(&mut overview).exit(),
                },
            }
        }
    }
}

/// Runs a Flow command and exits with its `error-mapping-v1.md` code.
async fn run_flow(cli: FlowCli) -> anyhow::Result<()> {
    let exit_code = cli_app::run(cli).await;
    std::process::exit(exit_code);
}

/// Runs a workspace command through the handler `mcp-server` uses. Its error is returned to
/// `main`, which reports it exactly as `mcp-server`'s `main` does.
async fn run_business(args: &[OsString]) -> anyhow::Result<()> {
    let cli = cli::parse_business_cli(args);
    cli::run_business(&cli.global, &cli.command).await
}
