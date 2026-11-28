//! The `sv-index` command: argument parsing and dispatch. Output goes to stdout (report) and
//! stderr (usage and I/O errors) by design.
//!
//! Exit codes: `0` success, `1` the repository has findings, `2` usage or I/O error.

#![allow(clippy::print_stdout, clippy::print_stderr)]

mod build;
mod check;
mod entry;
mod fetch;
mod repo;
mod report;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

const AFTER_HELP: &str = "\
Exit codes:
  0  success: no findings
  1  the index repository has findings (invalid document, stale index.json, rule violation,
     failed download or verification); each finding names its file and JSON path
  2  usage error or an I/O problem that prevented the run (unreadable repo, missing base)

CI: run `check` on the pull request without secrets and never pass --resolve or --ca-file there
(they skip the public-address rule and add trust roots). Symbolic links in the checkout are refused
and never followed.";

/// A failure that is not a finding: usage or I/O (exit code 2).
#[derive(Debug)]
pub struct CliError(pub String);

impl<T: std::fmt::Display> From<T> for CliError {
    fn from(e: T) -> Self {
        Self(e.to_string())
    }
}

#[derive(Parser)]
#[command(name = "sv-index", version, about = "Build and check the public plugin index repository", after_help = AFTER_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate `index.json` from `publishers/` and `plugins/` (offline; refuses invalid documents).
    Build(BuildArgs),
    /// Validate every document and image, `index.json` freshness, the append-only rules against a
    /// base, and (downloads) the package behind each new or changed version.
    Check(CheckArgs),
    /// Print the version entry (JSON) of a local package, ready to paste into `plugin.json`.
    Entry(EntryArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    /// Index repository checkout.
    repo: PathBuf,
    /// JSON file with `title`, optional `description` and `metadata` (categories and labels).
    /// Without it they are kept from the existing `index.json`.
    #[arg(long)]
    meta: Option<PathBuf>,
    /// Generation time (Unix seconds) to write when the content changed; default is now.
    #[arg(long)]
    generated_at: Option<u64>,
    /// Machine-readable report.
    #[arg(long)]
    json: bool,
}

/// Which package downloads `check` performs.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DownloadMode {
    /// No download.
    None,
    /// New versions and versions whose URL changed (needs a base).
    Changed,
    /// Every listed version.
    All,
}

#[derive(Args)]
pub struct CheckArgs {
    /// Index repository checkout (the head).
    repo: PathBuf,
    /// Base checkout directory whose `index.json` is the previous state.
    #[arg(long, group = "base_spec")]
    base: Option<PathBuf>,
    /// Git ref of `<repo>` whose `index.json` is the previous state (for example `origin/main`).
    #[arg(long, group = "base_spec")]
    base_ref: Option<String>,
    /// There is no previous state (the first commit): everything is new.
    #[arg(long, group = "base_spec")]
    base_empty: bool,
    /// Packages to download and verify. Default: `changed` with a base, else `none`.
    #[arg(long, value_enum)]
    download: Option<DownloadMode>,
    /// Exact download hosts (comma separated; every redirect hop is checked). Default: any public host.
    #[arg(long, value_delimiter = ',')]
    download_hosts: Vec<String>,
    /// Pin a host to an address, `host=ip[:port]` (repeatable). Pinned addresses skip the
    /// public-address rule and may use another port; for test indexes on a private network.
    /// Never pass this in CI.
    #[arg(long = "resolve", value_name = "HOST=IP[:PORT]")]
    resolve: Vec<String>,
    /// PEM file with extra root certificates to trust (repeatable); for test TLS. Never pass this in CI.
    #[arg(long = "ca-file")]
    ca_file: Vec<PathBuf>,
    /// Most downloads in one run; the rest are reported as not verified and the run fails.
    #[arg(long, default_value_t = 50)]
    max_downloads: usize,
    /// Machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct EntryArgs {
    /// Package file.
    package: PathBuf,
    /// Download URL (https).
    #[arg(long)]
    url: String,
    /// `sylvode` range; default `>=<sv.min of the manifest>`.
    #[arg(long)]
    sylvode: Option<String>,
    /// Publication time (Unix seconds); default is now.
    #[arg(long)]
    published_at: Option<u64>,
}

/// Runs the command line and maps the outcome to the exit code.
#[must_use]
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Build(a) => build::run(&a),
        Command::Check(a) => check::run(&a),
        Command::Entry(a) => entry::run(&a),
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(CliError(e)) => {
            eprintln!("sv-index: {e}");
            ExitCode::from(2)
        }
    }
}

/// Current Unix time in seconds.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
