#![allow(clippy::print_stdout)]

use std::env::{self, VarError};
use std::path::Path;
use std::process::Command;

const COMMIT_ENV: &str = "OPENPR_BUILD_GIT_COMMIT";
const DIRTY_ENV: &str = "OPENPR_BUILD_GIT_DIRTY";
const COMMITTER_DATE_ENV: &str = "OPENPR_BUILD_GIT_COMMITTER_DATE";

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed={COMMIT_ENV}");
    println!("cargo:rerun-if-env-changed={DIRTY_ENV}");
    println!("cargo:rerun-if-env-changed={COMMITTER_DATE_ENV}");

    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The metadata describes the entire Git work tree, including untracked files.
    // A tracked-file list cannot express that dependency. Cargo recursively scans
    // a directory input, so one repository-root dependency observes tracked and
    // untracked changes while a converged, unchanged build remains fresh.
    println!("cargo:rerun-if-changed={}", repo.display());

    let explicit_commit = optional_env(COMMIT_ENV)?;
    let explicit_dirty = optional_env(DIRTY_ENV)?;
    let explicit_committer_date = optional_env(COMMITTER_DATE_ENV)?;
    let (commit, dirty, committer_date, source) = match (explicit_commit, explicit_dirty, explicit_committer_date) {
        (None, None, None) => match git_metadata(&repo) {
            Ok(metadata) => metadata,
            Err(reason) => {
                println!("cargo:warning=build provenance is unknown: {reason}");
                unknown_metadata()
            }
        },
        (Some(commit), Some(dirty), Some(committer_date))
            if commit == "unknown" && dirty == "unknown" && committer_date == "unknown" =>
        {
            unknown_metadata()
        }
        (Some(commit), Some(dirty), Some(committer_date)) => {
            validate_commit(&commit)?;
            validate_dirty(&dirty)?;
            validate_committer_date(&committer_date)?;
            (commit, dirty, committer_date, "environment")
        }
        _ => {
            return Err(format!(
                "{COMMIT_ENV}, {DIRTY_ENV}, and {COMMITTER_DATE_ENV} must be supplied together"
            ));
        }
    };

    println!("cargo:rustc-env=OPENPR_EMBEDDED_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=OPENPR_EMBEDDED_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=OPENPR_EMBEDDED_GIT_COMMITTER_DATE={committer_date}");
    println!("cargo:rustc-env=OPENPR_EMBEDDED_PROVENANCE_SOURCE={source}");
    Ok(())
}

fn optional_env(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(format!("{name} is not valid Unicode")),
    }
}

fn unknown_metadata() -> (String, String, String, &'static str) {
    (
        "unknown".to_owned(),
        "unknown".to_owned(),
        "unknown".to_owned(),
        "unknown",
    )
}

fn git_metadata(repo: &Path) -> Result<(String, String, String, &'static str), String> {
    let commit = git(repo, &["rev-parse", "HEAD"])?;
    if !is_commit(&commit) {
        return Err(format!("git returned malformed HEAD {commit:?}"));
    }
    let committer_date = git(repo, &["show", "-s", "--format=%cI", "HEAD"])?;
    validate_committer_date(&committer_date)?;
    let status = git(repo, &["status", "--porcelain", "--untracked-files=normal"])?;
    let dirty = if status.is_empty() { "false" } else { "true" };
    Ok((commit, dirty.to_owned(), committer_date, "git"))
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        // Provenance queries must be observational. In particular, `git status`
        // may otherwise refresh .git/index, which would retrigger the repository
        // directory dependency on the next Cargo invocation.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("could not execute git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {} exited {}", args.join(" "), output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn validate_commit(value: &str) -> Result<(), String> {
    if is_commit(value) {
        Ok(())
    } else {
        Err(format!("{COMMIT_ENV} must be a lowercase 40-hex commit, got {value:?}"))
    }
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_dirty(value: &str) -> Result<(), String> {
    if matches!(value, "true" | "false") {
        Ok(())
    } else {
        Err(format!("{DIRTY_ENV} must be true or false, got {value:?}"))
    }
}

fn validate_committer_date(value: &str) -> Result<(), String> {
    // git `%cI` is strict ISO 8601. Git 2.56 on GitHub Actions prints UTC as `Z`
    // (`2026-10-31T12:05:33Z`); older git prints the same instant as `+00:00`.
    // Rejecting `Z` made the build fall back to commit `unknown`, and package
    // export then refused every job. Fractional seconds are accepted because the
    // provenance verifier already allows them.
    let Some((date, rest)) = value.split_once('T') else {
        return Err(committer_date_error(value));
    };
    if !is_yyyy_mm_dd(date) {
        return Err(committer_date_error(value));
    }
    let Some((clock, fraction_and_zone)) = split_clock(rest) else {
        return Err(committer_date_error(value));
    };
    if !is_hh_mm_ss(clock) {
        return Err(committer_date_error(value));
    }
    let zone = match fraction_and_zone.strip_prefix('.') {
        Some(after_dot) => {
            let Some((digits, zone)) = split_fraction(after_dot) else {
                return Err(committer_date_error(value));
            };
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(committer_date_error(value));
            }
            zone
        }
        None => fraction_and_zone,
    };
    if is_zone(zone) {
        Ok(())
    } else {
        Err(committer_date_error(value))
    }
}

fn committer_date_error(value: &str) -> String {
    format!("{COMMITTER_DATE_ENV} must be git's strict ISO 8601 committer date (%cI), got {value:?}")
}

fn is_yyyy_mm_dd(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && is_ascii_digits(bytes.get(..4))
        && is_ascii_digits(bytes.get(5..7))
        && is_ascii_digits(bytes.get(8..10))
}

fn is_hh_mm_ss(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 8
        && bytes.get(2) == Some(&b':')
        && bytes.get(5) == Some(&b':')
        && is_ascii_digits(bytes.get(..2))
        && is_ascii_digits(bytes.get(3..5))
        && is_ascii_digits(bytes.get(6..8))
}

fn split_clock(value: &str) -> Option<(&str, &str)> {
    if value.len() < 8 {
        return None;
    }
    let (clock, rest) = value.split_at_checked(8)?;
    Some((clock, rest))
}

fn split_fraction(value: &str) -> Option<(&str, &str)> {
    let zone_at = value.find(['Z', '+', '-'])?;
    let (digits, zone) = value.split_at_checked(zone_at)?;
    Some((digits, zone))
}

fn is_zone(value: &str) -> bool {
    if value == "Z" {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() == 6
        && matches!(bytes.first(), Some(b'+' | b'-'))
        && bytes.get(3) == Some(&b':')
        && is_ascii_digits(bytes.get(1..3))
        && is_ascii_digits(bytes.get(4..6))
}

fn is_ascii_digits(bytes: Option<&[u8]>) -> bool {
    bytes.is_some_and(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit))
}
