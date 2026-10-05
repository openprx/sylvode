//! Which parser one `sylvode` command line belongs to.
//!
//! `sylvode` carries two command models side by side (ADR-0020 D5): the Flow groups of
//! `cli-surface-v1.md` ([`super::command::Cli`]) and the nine workspace groups it shares with
//! `mcp-server` ([`crate::cli::BusinessCli`]). The two keep their own global options, their own
//! output conventions and their own exit codes, so they are not merged into one `clap` tree —
//! a merged tree would have to make `--workspace-id` either global, which changes what every
//! Flow command accepts, or local, which changes where the workspace commands accept it.
//! Instead the line is routed on its first subcommand name to the unchanged parser that owns
//! it, and only a line naming no known group (bare `sylvode`, `--help`, `help`, a typo) goes to
//! [`overview_command`], which lists all fifteen groups.

use super::command::Cli as FlowCli;
use crate::cli::{BUSINESS_GROUPS, BusinessCommands};
use clap::{CommandFactory, Subcommand};
use std::ffi::OsString;

/// The Flow groups' top-level names, as [`FlowCli`] defines them.
pub const FLOW_GROUPS: [&str; 6] = ["features", "objects", "collections", "records", "collab", "deliveries"];

/// Global options of either model that take a value in the next argument.
///
/// Skipped together with that value while looking for the first subcommand name, so
/// `sylvode --format table projects list` routes on `projects`, not on `table`.
const VALUE_OPTIONS: [&str; 5] = ["--config", "--format", "--api-url", "--bot-token", "--workspace-id"];

/// The parser a command line is handed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// One of [`FLOW_GROUPS`]: parsed and run exactly as before by [`FlowCli`].
    Flow,
    /// One of [`BUSINESS_GROUPS`]: parsed and run by the definitions and handler `mcp-server`
    /// uses.
    Business,
    /// No known group: help, usage errors and suggestions over all fifteen groups.
    Overview,
}

/// Classifies `args` (including the program name in position 0) by its first subcommand name.
///
/// `help <group>` routes like `<group>`, so `sylvode help projects` prints the same help as
/// `sylvode projects --help`.
pub fn route(args: &[OsString]) -> Route {
    let mut positionals = positionals(args.get(1..).unwrap_or_default());
    let first = positionals.next();
    let name = if first == Some("help") {
        positionals.next()
    } else {
        first
    };
    match name {
        Some(name) if FLOW_GROUPS.contains(&name) => Route::Flow,
        Some(name) if BUSINESS_GROUPS.contains(&name) => Route::Business,
        _ => Route::Overview,
    }
}

/// The arguments that are neither options nor option values, up to a `--` terminator.
fn positionals(args: &[OsString]) -> impl Iterator<Item = &str> {
    let mut skip_value = false;
    let mut terminated = false;
    args.iter().filter_map(move |arg| {
        if terminated {
            return None;
        }
        if std::mem::take(&mut skip_value) {
            return None;
        }
        // A non-UTF-8 argument is never a group name; it still ends option scanning the way
        // a positional does, so it is reported as an unmatched name.
        let Some(arg) = arg.to_str() else {
            return Some("");
        };
        if arg == "--" {
            terminated = true;
            return None;
        }
        if arg.starts_with('-') {
            skip_value = !arg.contains('=') && VALUE_OPTIONS.contains(&arg);
            return None;
        }
        Some(arg)
    })
}

/// The `sylvode` top-level command as help and usage errors present it: every Flow group,
/// then every workspace group.
pub fn overview_command() -> clap::Command {
    BusinessCommands::augment_subcommands(FlowCli::command())
        .about("Sylvode Flow CLI and workspace commands")
        .after_help(
            "Flow commands (features, objects, collections, records, collab, deliveries) follow the \
             sylvode.cli.v1 JSON contract. Workspace commands (projects, work-items, comments, labels, \
             sprints, search, files, operation-logs, tools) are the same commands mcp-server provides, \
             with the same output and exit codes, and also accept --workspace-id. Run \
             `sylvode <COMMAND> --help` for the options of one command.",
        )
}

#[cfg(test)]
mod tests {
    use super::{FLOW_GROUPS, Route, overview_command, route};
    use crate::cli::BUSINESS_GROUPS;
    use crate::cli_app::command::Cli as FlowCli;
    use clap::CommandFactory;
    use std::ffi::OsString;

    fn routed(args: &[&str]) -> Route {
        let args: Vec<OsString> = std::iter::once("sylvode")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect();
        route(&args)
    }

    #[test]
    fn the_flow_group_list_matches_the_flow_parser() {
        let names: Vec<String> = FlowCli::command()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .collect();
        assert_eq!(names, FLOW_GROUPS);
    }

    #[test]
    fn every_group_routes_to_the_parser_that_defines_it() {
        for group in FLOW_GROUPS {
            assert_eq!(routed(&[group, "--help"]), Route::Flow, "{group}");
            assert_eq!(routed(&["help", group]), Route::Flow, "{group}");
        }
        for group in BUSINESS_GROUPS {
            assert_eq!(routed(&[group, "--help"]), Route::Business, "{group}");
            assert_eq!(routed(&["help", group]), Route::Business, "{group}");
        }
    }

    #[test]
    fn option_values_before_the_group_are_not_mistaken_for_it() {
        assert_eq!(
            routed(&["--format", "table", "--config", "x.toml", "projects", "list"]),
            Route::Business
        );
        assert_eq!(
            routed(&["--workspace-id", "objects", "projects", "list"]),
            Route::Business
        );
        assert_eq!(routed(&["--format=table", "objects", "get", "x"]), Route::Flow);
        assert_eq!(
            routed(&["--api-url", "http://a", "collab", "inspect", "x"]),
            Route::Flow
        );
    }

    #[test]
    fn lines_naming_no_known_group_go_to_the_overview() {
        assert_eq!(routed(&[]), Route::Overview);
        assert_eq!(routed(&["--help"]), Route::Overview);
        assert_eq!(routed(&["help"]), Route::Overview);
        assert_eq!(routed(&["serve"]), Route::Overview);
        assert_eq!(routed(&["projectz", "list"]), Route::Overview);
        assert_eq!(routed(&["--", "projects"]), Route::Overview);
    }

    #[test]
    fn the_overview_lists_all_fifteen_groups_and_no_serve() {
        let names: Vec<String> = overview_command()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .collect();
        let expected: Vec<&str> = FLOW_GROUPS.iter().chain(BUSINESS_GROUPS.iter()).copied().collect();
        assert_eq!(names, expected);
    }
}
