//! `sv-index`: build and check the public plugin index repository (contract M15 §4).
//!
//! ```text
//! sv-index build <repo> [--meta <file>] [--generated-at <secs>] [--json]
//! sv-index check <repo> [--base <dir> | --base-ref <ref> | --base-empty] [--download none|changed|all] [--json]
//! sv-index entry <package.svpkg> --url <https-url> [--sylvode <range>] [--published-at <secs>]
//! ```
//!
//! Exit codes: 0 success, 1 the index repository has findings, 2 usage or I/O error.

use std::process::ExitCode;

fn main() -> ExitCode {
    sv_index::cli::run()
}
