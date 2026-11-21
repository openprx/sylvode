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
const SYSTEM_PARENTS: [&str; 16] = [
    "/var",
    "/private/var",
    "/home",
    "/Users",
    "/tmp",
    "/private/tmp",
    "/root",
    "/mnt",
    "/Volumes",
    "/Applications",
    "/run",
    "/srv",
    "/nix",
    "/snap",
    "/media",
    "/cores",
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
    roots: Vec<Root>,
}

/// One root: as the operator wrote it (expanded) and canonical. A directory from Sylvode must lie
/// lexically under one of the two before the file system is touched for it.
#[derive(Clone, Debug)]
struct Root {
    original: PathBuf,
    canonical: PathBuf,
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
        // Without a known home directory the "not the home directory" rule cannot be applied, so no
        // root is accepted.
        let canonical_home = home
            .and_then(|home| home.canonicalize().ok())
            .ok_or_else(|| "runner.allowed_roots cannot be checked: the home directory is not known".to_string())?;
        let mut roots: Vec<Root> = Vec::new();
        let mut warnings = Vec::new();
        for entry in entries {
            let root = validate_root(entry, home, Some(&canonical_home))?;
            let mode = std::fs::metadata(&root.canonical).map_or(0, |m| m.permissions().mode());
            let world_writable = mode & 0o002 != 0;
            let sticky = mode & 0o1000 != 0;
            if world_writable && !sticky {
                warnings.push(format!(
                    "runner.allowed_roots entry `{entry}` is world-writable: any local user can place a \
                     directory inside it that a project may then use"
                ));
            }
            if !roots.iter().any(|known| known.canonical == root.canonical) {
                roots.push(root);
            }
        }
        Ok(RootsReport {
            roots: Self { roots },
            warnings,
        })
    }

    /// Whether `path` is equal to, inside, or an ancestor of any root (compared as written and
    /// canonical).
    pub fn overlaps(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| {
            [&root.original, &root.canonical]
                .into_iter()
                .any(|root| path.starts_with(root) || root.starts_with(path))
        })
    }

    /// Whether any root of `self` overlaps any root of `other`.
    pub fn overlaps_roots(&self, other: &Self) -> bool {
        self.roots
            .iter()
            .any(|mine| other.overlaps(&mine.canonical) || other.overlaps(&mine.original))
    }

    /// Checks the working directory Sylvode sent and returns the canonical path to use.
    ///
    /// The order matters, because the answer reaches workspace members: first the syntax, then
    /// whether the path lies lexically under an allowed root (as the operator wrote the root, or
    /// canonical); a path that does not is refused as `path_outside_allowlist` WITHOUT the file
    /// system being touched, so nothing about the host outside the roots can be learned from the
    /// answer. Only inside a root is existence, readability and type probed and reported. After
    /// `canonicalize` (which resolves every link) containment is decided again on the real path.
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
        let given = Path::new(raw);
        if !self
            .roots
            .iter()
            .any(|root| lexically_under(given, &root.original) || lexically_under(given, &root.canonical))
        {
            return Err(OUTSIDE);
        }
        let canonical = match given.canonicalize() {
            Ok(path) => path,
            Err(e) => {
                // A link inside a root can lead out of it: if the nearest part that exists is
                // outside every root, the answer is the same as for any outside path.
                if !self.nearest_existing_is_inside(given) {
                    return Err(OUTSIDE);
                }
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
        // Components, not bytes: `/work/svwl-evil` is not under `/work/svwl`.
        let inside = self.roots.iter().any(|root| canonical.starts_with(&root.canonical));
        // A path that is not valid UTF-8 cannot be handed on as text without becoming another path.
        let plain = canonical
            .to_str()
            .is_some_and(|text| !text.contains('#') && !text.chars().any(char::is_control));
        if !inside || !plain {
            return Err(OUTSIDE);
        }
        if !canonical.is_dir() {
            return Err(PathRefusal {
                code: "path_not_directory",
                reason: "the project working directory is not a directory",
            });
        }
        Ok(canonical)
    }

    /// Whether the longest existing prefix of `path` resolves to somewhere inside a root. A
    /// symbolic link on the way (even a dangling one) is followed, so a link out of a root is
    /// seen for what it is.
    fn nearest_existing_is_inside(&self, path: &Path) -> bool {
        self.inside_following_links(path, 0)
    }

    fn inside_following_links(&self, path: &Path, depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        let mut current = Some(path);
        while let Some(candidate) = current {
            if let Ok(meta) = std::fs::symlink_metadata(candidate)
                && meta.file_type().is_symlink()
            {
                let Ok(target) = std::fs::read_link(candidate) else {
                    return false;
                };
                let resolved = if target.is_absolute() {
                    target
                } else {
                    candidate
                        .parent()
                        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
                        .join(target)
                };
                return self.inside_following_links(&resolved, depth + 1);
            }
            if let Ok(resolved) = candidate.canonicalize() {
                return self.roots.iter().any(|root| resolved.starts_with(&root.canonical));
            }
            current = candidate.parent();
        }
        false
    }
}

/// Whether `path` lies under `root`, component by component.
fn lexically_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// Whether two paths are the same, as the file system sees it: case-insensitively on macOS, whose
/// usual volumes ignore case.
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(target_os = "macos") {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Whether `path` is `base` or lies below it, with [`same_path`]'s notion of equality.
fn at_or_below(path: &Path, base: &Path) -> bool {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if same_path(candidate, base) {
            return true;
        }
        current = candidate.parent();
    }
    false
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

fn validate_root(entry: &str, home: Option<&Path>, canonical_home: Option<&Path>) -> Result<Root, String> {
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
        && at_or_below(home, &canonical)
    {
        return Err(name("must not be the home directory or a directory that contains it"));
    }
    for tree in SYSTEM_TREES {
        let tree = Path::new(tree);
        let resolved = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
        for candidate in [tree, resolved.as_path()] {
            if at_or_below(&canonical, candidate) || at_or_below(candidate, &canonical) {
                return Err(name("is, contains or lies inside a system directory"));
            }
        }
    }
    for parent in SYSTEM_PARENTS {
        let parent = Path::new(parent);
        let resolved = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
        if same_path(&canonical, parent) || same_path(&canonical, &resolved) {
            return Err(name("is a directory that holds other users' or system data"));
        }
    }
    if canonical.to_str().is_none() {
        return Err(name("must be a valid UTF-8 path"));
    }
    Ok(Root {
        original: expanded,
        canonical,
    })
}

/// The home directory of the user the runner runs as: `HOME`, else the passwd entry of the user.
pub fn service_user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::Uid::effective())
                .ok()
                .flatten()
                .map(|user| user.dir)
        })
}
