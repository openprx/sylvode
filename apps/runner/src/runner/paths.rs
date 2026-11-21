//! The allow-list of root directories (design section 6.2) and the per-delivery path check.
//!
//! Roots are validated once at start-up: `~/` is expanded (no other `~` form), the path must be
//! absolute, exist, be a directory and be canonicalised; `/`, the home directory (or any
//! directory that contains it), system directories and anything with `#` in its path (tmux reads
//! `-c` as a format) are refused. A working directory from Sylvode is checked again for every
//! delivery: syntax, then `canonicalize` (which resolves every symbolic link), then containment by
//! path components (`/work/svwl-evil` is not under `/work/svwl`). Only the canonical path is used
//! afterwards; the string Sylvode sent is never used again.
//!
//! A directory swapped for a symbolic link between this check and the use is out of scope: only a
//! process that can write inside an allowed root (the same OS user) can do that.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The longest working directory path accepted.
pub const MAX_PATH_BYTES: usize = 1024;

/// Directories that are never roots, nor inside one, nor a parent of one.
const SYSTEM_TREES: [&str; 14] = [
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/System",
    "/Library",
    "/private/etc",
    "/opt/homebrew",
];

/// Directories that are never roots themselves (they hold other people's data), though a
/// directory below them is fine.
const SYSTEM_PARENTS: [&str; 8] = [
    "/var",
    "/private/var",
    "/home",
    "/Users",
    "/tmp",
    "/private/tmp",
    "/root",
    "/mnt",
];

/// Why a working directory was declined, with the code of the refusal answer.
#[derive(Debug, PartialEq, Eq)]
pub struct PathRefusal {
    pub code: &'static str,
    /// One line for Sylvode: never contains the path.
    pub reason: &'static str,
}

/// The validated roots, canonical.
#[derive(Clone, Debug, Default)]
pub struct AllowedRoots {
    roots: Vec<PathBuf>,
}

/// Roots and what was worth a warning while validating them.
#[derive(Debug)]
pub struct RootsReport {
    pub roots: AllowedRoots,
    pub warnings: Vec<String>,
}

impl AllowedRoots {
    /// Validates every entry of `allowed_roots`. `home` is the service user's home directory.
    ///
    /// # Errors
    ///
    /// A message naming the entry; an invalid root refuses the `runner` agents.
    pub fn from_entries(entries: &[String], home: Option<&Path>) -> Result<RootsReport, String> {
        let canonical_home = home.and_then(|home| home.canonicalize().ok());
        let mut roots = Vec::new();
        let mut warnings = Vec::new();
        for entry in entries {
            let root = validate_root(entry, home, canonical_home.as_deref())?;
            let mode = std::fs::metadata(&root).map_or(0, |m| m.permissions().mode());
            let world_writable = mode & 0o002 != 0;
            let sticky = mode & 0o1000 != 0;
            if world_writable && !sticky {
                warnings.push(format!(
                    "runner.allowed_roots entry `{entry}` is world-writable: any local user can place a \
                     directory inside it that a project may then use"
                ));
            }
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        Ok(RootsReport {
            roots: Self { roots },
            warnings,
        })
    }

    /// Checks the working directory Sylvode sent and returns the canonical path to use.
    ///
    /// # Errors
    ///
    /// The refusal code and a path-free reason.
    pub fn check_directory(&self, raw: &str) -> Result<PathBuf, PathRefusal> {
        const OUTSIDE: PathRefusal = PathRefusal {
            code: "path_outside_allowlist",
            reason: "the project working directory is not under any allowed root of this runner",
        };
        if !syntactically_acceptable(raw) {
            return Err(OUTSIDE);
        }
        let canonical = match Path::new(raw).canonicalize() {
            Ok(path) => path,
            Err(e) => {
                return Err(match e.kind() {
                    ErrorKind::NotFound => PathRefusal {
                        code: "path_missing",
                        reason: "the project working directory does not exist on the runner host",
                    },
                    ErrorKind::PermissionDenied => PathRefusal {
                        code: "path_unreadable",
                        reason: "the project working directory cannot be read by the runner user",
                    },
                    // Not a directory in the middle of the path, a loop, a name too long.
                    _ => PathRefusal {
                        code: "path_not_directory",
                        reason: "the project working directory is not a directory",
                    },
                });
            }
        };
        if !canonical.is_dir() {
            return Err(PathRefusal {
                code: "path_not_directory",
                reason: "the project working directory is not a directory",
            });
        }
        // Components, not bytes: `/work/svwl-evil` is not under `/work/svwl`.
        let inside = self.roots.iter().any(|root| canonical.starts_with(root));
        if !inside || canonical.to_string_lossy().contains('#') || has_control(&canonical) {
            return Err(OUTSIDE);
        }
        Ok(canonical)
    }
}

fn has_control(path: &Path) -> bool {
    path.to_string_lossy().chars().any(char::is_control)
}

/// The API's syntactic checks, repeated: absolute, no NUL or control characters, no `.` or `..`
/// segment, no empty segment, not too long; and no `#`, which tmux would read as a format.
fn syntactically_acceptable(raw: &str) -> bool {
    if !raw.starts_with('/') || raw.len() > MAX_PATH_BYTES || raw.chars().any(char::is_control) || raw.contains('#') {
        return false;
    }
    if raw.len() > 1 && raw.ends_with('/') {
        return false;
    }
    raw.split('/')
        .skip(1)
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
        || raw == "/"
}

fn validate_root(entry: &str, home: Option<&Path>, canonical_home: Option<&Path>) -> Result<PathBuf, String> {
    let name = |why: &str| format!("runner.allowed_roots entry `{entry}`: {why}");
    let expanded = if let Some(rest) = entry.strip_prefix("~/") {
        let home = home.ok_or_else(|| name("`~/` needs a home directory and none is known"))?;
        home.join(rest)
    } else if entry.starts_with('~') {
        return Err(name("only a leading `~/` is expanded"));
    } else {
        PathBuf::from(entry)
    };
    if !expanded.is_absolute() {
        return Err(name("must be an absolute path"));
    }
    if entry.chars().any(char::is_control) {
        return Err(name("must not contain control characters"));
    }
    let canonical = expanded
        .canonicalize()
        .map_err(|e| name(&format!("cannot be used ({})", e.kind())))?;
    if !canonical.is_dir() {
        return Err(name("is not a directory"));
    }
    if canonical.to_string_lossy().contains('#') {
        return Err(name("must not contain `#` (tmux would read it as a format)"));
    }
    if canonical == Path::new("/") {
        return Err(name("must not be `/`"));
    }
    if let Some(home) = canonical_home
        && home.starts_with(&canonical)
    {
        return Err(name("must not be the home directory or a directory that contains it"));
    }
    for tree in SYSTEM_TREES {
        let tree = Path::new(tree);
        let resolved = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
        for candidate in [tree, resolved.as_path()] {
            if canonical.starts_with(candidate) || candidate.starts_with(&canonical) {
                return Err(name("is, contains or lies inside a system directory"));
            }
        }
    }
    for parent in SYSTEM_PARENTS {
        let parent = Path::new(parent);
        let resolved = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
        if canonical == parent || canonical == resolved {
            return Err(name("is a directory that holds other users' or system data"));
        }
    }
    Ok(canonical)
}
