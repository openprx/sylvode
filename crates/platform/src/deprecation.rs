//! The user-visible deprecation notices of ADR-0020 D2, defined in one place.
//!
//! Every notice names three things: the legacy name in use, its replacement, and the earliest
//! release that may remove it — with the product named, "Sylvode v2.0". A notice missing any
//! of the three does not satisfy D2, so the texts are built here and nowhere else; callers
//! only choose where a notice is written (stderr, a `warn` log line, a JSON-RPC `_meta`).

/// The earliest release that may remove a legacy entry point, as `_meta.deprecation` reports it.
pub const EARLIEST_REMOVAL: &str = "2.0";

/// The removal clause every notice ends with.
pub const NOT_REMOVED_BEFORE: &str = "not removed before Sylvode v2.0";

/// Written to stderr, once per process, when `mcp-server` runs a subcommand other than `serve`.
///
/// `subcommand` is the subcommand path as typed (`projects list`, `search`), never its argument
/// values, which may carry a token.
pub fn legacy_cli_invocation(subcommand: &str) -> String {
    format!(
        "warning: `mcp-server {subcommand}` is deprecated; use `sylvode {subcommand}` instead \
         (the mcp-server CLI subcommands are {NOT_REMOVED_BEFORE})"
    )
}

#[cfg(test)]
mod tests {
    use super::{EARLIEST_REMOVAL, NOT_REMOVED_BEFORE, legacy_cli_invocation};

    #[test]
    fn the_cli_notice_names_the_legacy_command_its_replacement_and_the_removal_release() {
        let notice = legacy_cli_invocation("work-items get");
        assert!(notice.contains("`mcp-server work-items get`"), "{notice}");
        assert!(notice.contains("`sylvode work-items get`"), "{notice}");
        assert!(notice.contains("not removed before Sylvode v2.0"), "{notice}");
        assert!(!notice.contains('\n'), "the notice must be one line: {notice}");
    }

    #[test]
    fn the_removal_release_is_v2() {
        assert_eq!(EARLIEST_REMOVAL, "2.0");
        assert_eq!(NOT_REMOVED_BEFORE, "not removed before Sylvode v2.0");
    }
}
