//! The user-visible deprecation notices of ADR-0020 D2, defined in one place.
//!
//! Every notice names three things: the legacy name in use, its replacement, and the earliest
//! release that may remove it — with the product named, "Sylvode v2.0". A notice missing any
//! of the three does not satisfy D2, so the texts are built here and nowhere else; callers
//! only choose where a notice is written (stderr, a `warn` log line, a JSON-RPC `_meta`).
//!
//! The shell scripts cannot link this crate. `scripts/lib/sylvode_compat.sh` builds its notices
//! with the same [`NOT_REMOVED_BEFORE`] clause, and the tests here run that library and fail if
//! the two drift apart.

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

/// Logged at `warn`, once per process, when the legacy configuration file was picked up by
/// default discovery rather than named with `--config`.
pub fn legacy_config_discovery(legacy: &str, canonical: &str) -> String {
    format!(
        "legacy configuration file {legacy} was discovered by default; rename it to {canonical} \
         or pass --config explicitly (default discovery of {legacy} is {NOT_REMOVED_BEFORE})"
    )
}

#[cfg(test)]
mod tests {
    use super::{EARLIEST_REMOVAL, NOT_REMOVED_BEFORE, legacy_cli_invocation, legacy_config_discovery};

    #[test]
    fn the_cli_notice_names_the_legacy_command_its_replacement_and_the_removal_release() {
        let notice = legacy_cli_invocation("work-items get");
        assert!(notice.contains("`mcp-server work-items get`"), "{notice}");
        assert!(notice.contains("`sylvode work-items get`"), "{notice}");
        assert!(notice.contains("not removed before Sylvode v2.0"), "{notice}");
        assert!(!notice.contains('\n'), "the notice must be one line: {notice}");
    }

    #[test]
    fn the_config_notice_names_the_legacy_file_its_replacement_and_the_removal_release() {
        let notice = legacy_config_discovery("config/openpr.toml", "config/sylvode.toml");
        assert!(notice.contains("config/openpr.toml"), "{notice}");
        assert!(notice.contains("config/sylvode.toml"), "{notice}");
        assert!(notice.contains("not removed before Sylvode v2.0"), "{notice}");
        assert!(!notice.contains('\n'), "the notice must be one line: {notice}");
    }

    #[test]
    fn the_removal_release_is_v2() {
        assert_eq!(EARLIEST_REMOVAL, "2.0");
        assert_eq!(NOT_REMOVED_BEFORE, "not removed before Sylvode v2.0");
    }

    /// Runs `script` in `bash -c` under `set -euo pipefail` with the compatibility library
    /// sourced, from `dir`, and returns (exit code, stdout, stderr).
    #[cfg(unix)]
    fn shell(dir: &std::path::Path, script: &str) -> (Option<i32>, String, String) {
        let library = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/lib/sylvode_compat.sh");
        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("set -euo pipefail\nsource '{library}'\n{script}"))
            .current_dir(dir)
            .env_remove("OPENPR_API_PORT")
            .env_remove("SYLVODE_API_PORT")
            .env_remove("OPENPR_BIND_HOST")
            .env_remove("SYLVODE_BIND_HOST")
            .output();
        match output {
            Ok(output) => (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ),
            Err(error) => (None, String::new(), format!("bash could not run: {error}")),
        }
    }

    #[cfg(unix)]
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sylvode-compat-shell-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(dir.join("config")).unwrap_or_default();
        dir
    }

    /// A legacy compose variable resolves as before on stdout and reports itself once on
    /// stderr; a canonical one reports nothing.
    #[cfg(unix)]
    #[test]
    fn the_shell_library_reports_legacy_compose_variables_on_stderr_only() {
        let dir = scratch("env");
        let (code, stdout, stderr) = shell(
            &dir,
            "printf 'OPENPR_API_PORT=18081\\n' > env\nport=$(sylvode_resolve_env env SYLVODE_API_PORT OPENPR_API_PORT 8081)\necho \"port=$port\"",
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, "port=18081\n");
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(lines.len(), 1, "{stderr}");
        let notice = lines.first().copied().unwrap_or_default();
        assert!(notice.contains("OPENPR_API_PORT"), "{notice}");
        assert!(notice.contains("use SYLVODE_API_PORT"), "{notice}");
        assert!(notice.contains("not removed before Sylvode v2.0"), "{notice}");
        assert!(
            notice.contains(NOT_REMOVED_BEFORE),
            "the shell clause drifted from the binaries': {notice}"
        );

        let (code, stdout, stderr) = shell(
            &dir,
            "printf 'SYLVODE_API_PORT=28081\\n' > env\nport=$(sylvode_resolve_env env SYLVODE_API_PORT OPENPR_API_PORT 8081)\necho \"port=$port\"",
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, "port=28081\n");
        assert_eq!(stderr, "", "a canonical variable was reported");

        let (code, _, stderr) = shell(
            &dir,
            "export OPENPR_BIND_HOST=0.0.0.0\nhost=$(sylvode_resolve_env missing SYLVODE_BIND_HOST OPENPR_BIND_HOST 127.0.0.1)\n[[ $host == 0.0.0.0 ]]",
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert!(
            stderr.contains("OPENPR_BIND_HOST") && stderr.contains("use SYLVODE_BIND_HOST"),
            "{stderr}"
        );
        std::fs::remove_dir_all(dir).unwrap_or_default();
    }

    /// Resolved in the calling shell, each distinct legacy name is reported once per run.
    #[cfg(unix)]
    #[test]
    fn the_shell_library_reports_each_legacy_name_once_per_run() {
        let dir = scratch("dedupe");
        let (code, stdout, stderr) = shell(
            &dir,
            "printf 'OPENPR_API_PORT=18081\\nOPENPR_BIND_HOST=0.0.0.0\\n' > env\n\
             sylvode_resolve_env_into a env SYLVODE_API_PORT OPENPR_API_PORT 8081\n\
             sylvode_resolve_env_into b env SYLVODE_API_PORT OPENPR_API_PORT 8081\n\
             sylvode_resolve_env_into c env SYLVODE_BIND_HOST OPENPR_BIND_HOST 127.0.0.1\n\
             echo \"$a $b $c\"",
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, "18081 18081 0.0.0.0\n");
        assert_eq!(
            stderr.lines().filter(|line| line.contains("OPENPR_API_PORT")).count(),
            1,
            "{stderr}"
        );
        assert_eq!(
            stderr.lines().filter(|line| line.contains("OPENPR_BIND_HOST")).count(),
            1,
            "{stderr}"
        );
        assert_eq!(stderr.lines().count(), 2, "{stderr}");
        std::fs::remove_dir_all(dir).unwrap_or_default();
    }

    /// A legacy compose configuration file picked by default discovery is reported with its
    /// replacement; the canonical file is not.
    #[cfg(unix)]
    #[test]
    fn the_shell_library_reports_a_discovered_legacy_compose_config() {
        let dir = scratch("config");
        let (code, stdout, stderr) = shell(
            &dir,
            ": > config/openpr.compose.toml\nsylvode_select_config config/sylvode.compose.toml config/openpr.compose.toml",
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, "config/openpr.compose.toml\n");
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(lines.len(), 1, "{stderr}");
        let notice = lines.first().copied().unwrap_or_default();
        assert!(notice.contains("config/openpr.compose.toml"), "{notice}");
        assert!(notice.contains("use config/sylvode.compose.toml"), "{notice}");
        assert!(notice.contains("not removed before Sylvode v2.0"), "{notice}");

        let (code, stdout, stderr) = shell(
            &dir,
            "rm config/openpr.compose.toml\n: > config/sylvode.compose.toml\nsylvode_select_config config/sylvode.compose.toml config/openpr.compose.toml",
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, "config/sylvode.compose.toml\n");
        assert_eq!(stderr, "", "the canonical file was reported");
        std::fs::remove_dir_all(dir).unwrap_or_default();
    }

    /// A notice that cannot be written never fails a `set -euo pipefail` caller.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_stderr_does_not_fail_the_shell_caller() {
        let dir = scratch("stderr");
        for redirect in ["exec 2>&-", "exec 2>/dev/full"] {
            let (code, stdout, _) = shell(
                &dir,
                &format!(
                    "printf 'OPENPR_API_PORT=18081\\n' > env\n{redirect}\n\
                     sylvode_resolve_env_into a env SYLVODE_API_PORT OPENPR_API_PORT 8081\n\
                     b=$(sylvode_resolve_env env SYLVODE_API_PORT OPENPR_API_PORT 8081)\necho \"$a $b\""
                ),
            );
            assert_eq!(code, Some(0), "{redirect} failed the caller");
            assert_eq!(stdout, "18081 18081\n", "{redirect}");
        }
        std::fs::remove_dir_all(dir).unwrap_or_default();
    }
}
