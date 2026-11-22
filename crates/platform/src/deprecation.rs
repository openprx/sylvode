//! The user-visible notice for the `mcp-server` workspace subcommands, defined in one place.
//!
//! `mcp-server <command>` (any subcommand other than `serve`) still runs, but `sylvode <command>`
//! is the CLI. The notice names the command as typed and its replacement; callers only choose
//! where it is written.

/// Written to stderr, once per process, when `mcp-server` runs a subcommand other than `serve`.
///
/// `subcommand` is the subcommand path as typed (`projects list`, `search`), never its argument
/// values, which may carry a token.
pub fn legacy_cli_invocation(subcommand: &str) -> String {
    format!("warning: `mcp-server {subcommand}` is deprecated; use `sylvode {subcommand}` instead")
}

#[cfg(test)]
mod tests {
    use super::legacy_cli_invocation;

    #[test]
    fn the_cli_notice_names_the_command_as_typed_and_its_replacement() {
        let notice = legacy_cli_invocation("work-items get");
        assert!(notice.contains("`mcp-server work-items get`"), "{notice}");
        assert!(notice.contains("`sylvode work-items get`"), "{notice}");
        assert!(!notice.contains('\n'), "the notice must be one line: {notice}");
    }
}
