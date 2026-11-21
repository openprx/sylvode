//! Installing the agent guide into a project (design sections 4.10 and 8.5).
//!
//! - **codex and opencode:** a managed section in the project's `AGENTS.md`.
//! - **claude:** the skill directory `.claude/skills/sylvode-mcp/` with `SKILL.md`,
//!   `reference/tools.md` and `.sylvode-managed`, plus a `.git/info/exclude` line.
//!
//! The project is repository content that other people (and the agent itself) can change, so every
//! file operation here is relative to a directory handle and refuses symbolic links at every
//! component (`O_NOFOLLOW`), opens files without blocking and checks the file type on the opened
//! handle (a FIFO, device or socket is refused), caps every read, creates files exclusively and
//! writes them with fixed modes (set-id bits are never copied).
//!
//! "Unchanged" is decided by content: the managed block or the three skill files (and no other
//! entry in the skill directory) must equal what would be written; a forged stamp over other
//! content is replaced, with a backup. A skill directory without `.sylvode-managed` is never
//! touched. Bytes outside the managed markers are never changed.
//!
//! Backups go to the runner's state directory with exclusive creation (a second backup under the
//! same label gets a numeric suffix), the newest [`MAX_BACKUPS`] of each kind are kept, and a skill
//! directory larger than the backup caps is replaced with an incomplete backup
//! ([`Outcome::ReplacedBackupIncomplete`]).
//!
//! The content comes in through [`GuideContent`]; the generated files of the skills source of truth
//! (`skills/sylvode-mcp`, work package P3.6) are not in the tree yet, so no embedded constructor
//! exists here. It is added where the generator's output lands.

use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use sha2::{Digest, Sha256};

const BEGIN_PREFIX: &str = "<!-- BEGIN SYLVODE MANAGED SECTION";
const END_MARKER: &str = "<!-- END SYLVODE MANAGED SECTION -->";
const BEGIN_SUFFIX: &str = ". Do not edit: sylvode-runner replaces this block. -->";

/// The line added to `.git/info/exclude` for the claude skill directory.
pub const EXCLUDE_LINE: &str = ".claude/skills/sylvode-mcp/";

/// The most bytes of `AGENTS.md` that are read; a larger file is a conflict and is not touched.
pub const AGENTS_MAX_BYTES: u64 = 1024 * 1024;
/// The most bytes of one skill file that are read.
pub const SKILL_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// The most bytes of `.sylvode-managed` that are read.
pub const MARKER_MAX_BYTES: u64 = 4 * 1024;
/// The most bytes of `.git/info/exclude` that are read.
pub const EXCLUDE_MAX_BYTES: u64 = 64 * 1024;
/// How many backups of each kind are kept.
pub const MAX_BACKUPS: usize = 10;
/// The caps of one skill-directory backup: entries, bytes per file, bytes in total, depth.
pub const BACKUP_MAX_ENTRIES: usize = 64;
pub const BACKUP_MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const BACKUP_MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const BACKUP_MAX_DEPTH: usize = 3;

/// The text of the guide, as generated from the skills source of truth.
#[derive(Clone, Copy, Debug)]
pub struct GuideContent<'a> {
    /// `SKILL.md` (frontmatter included).
    pub skill_md: &'a str,
    /// `reference/tools.md`.
    pub tools_md: &'a str,
    /// The body of the managed `AGENTS.md` section.
    pub agents_section: &'a str,
}

/// What an install did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The file or directory was created, or the section was appended.
    Created,
    /// Different content was replaced, after a backup.
    Replaced,
    /// Different content was replaced, but the old skill directory was too large to back up in full.
    ReplacedBackupIncomplete,
    /// The content equals what would be written; nothing was written.
    Unchanged,
    /// The marker lines are not exactly one BEGIN followed by one END, or the file is over the size
    /// cap; nothing was written. The task still runs, because the server instructions carry the
    /// procedure too. `doctor` reports `agent_guide: conflict`.
    Conflict,
    /// The target is not ours or not safe to use: a skill directory without `.sylvode-managed`, a
    /// symbolic link, or a file that is not a regular file. It was not touched.
    NotManaged,
}

/// What happened to the git exclude line.
#[derive(Debug, PartialEq, Eq)]
pub enum ExcludeOutcome {
    /// The line was added to `.git/info/exclude`.
    Added,
    /// The line was already there.
    Present,
    /// The project is not a git checkout; nothing to do.
    NoGit,
    /// `.git` is a file (a worktree): the exclude file lives in the main repository's git
    /// directory, which this installer does not write to. `doctor` shows a warning.
    Worktree,
    /// A symbolic link, an oversized or non-regular file was in the way; nothing was written.
    Refused,
}

impl ExcludeOutcome {
    /// The `doctor` warning item for this outcome, if any: the skill directory could be committed.
    #[must_use]
    pub const fn doctor_warning(&self) -> Option<&'static str> {
        match self {
            Self::Worktree => Some(
                "agent_guide: not_excluded (git worktree; add .claude/skills/sylvode-mcp/ to the repository's exclude file)",
            ),
            Self::Refused => Some("agent_guide: not_excluded (the git exclude file could not be used safely)"),
            Self::Added | Self::Present | Self::NoGit => None,
        }
    }
}

fn io_err(what: &str, error: &std::io::Error) -> String {
    format!("{what}: {}", error.kind())
}

fn errno_err(what: &str, errno: Errno) -> String {
    format!("{what}: {errno}")
}

fn hex12(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes)).chars().take(12).collect()
}

/// `v=<version> h=<first 12 hex of the SHA-256 of what was installed>`.
#[must_use]
pub fn stamp(version: &str, hashed: &[u8]) -> String {
    format!("v={version} h={}", hex12(hashed))
}

fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= 64 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ----- directory-relative file access ---------------------------------------------------------

const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// What is at a name inside a directory.
pub(super) enum Entry {
    Absent,
    Dir,
    File,
    /// A symbolic link, FIFO, device, socket.
    Other,
}

pub(super) fn classify(parent: &impl AsFd, name: &str) -> Result<Entry, String> {
    match fstatat(parent, name, nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let kind = SFlag::from_bits_truncate(stat.st_mode as nix::libc::mode_t) & SFlag::S_IFMT;
            Ok(if kind == SFlag::S_IFDIR {
                Entry::Dir
            } else if kind == SFlag::S_IFREG {
                Entry::File
            } else {
                Entry::Other
            })
        }
        Err(Errno::ENOENT) => Ok(Entry::Absent),
        Err(errno) => Err(errno_err("could not inspect a path", errno)),
    }
}

pub(super) fn open_root(project_dir: &Path) -> Result<OwnedFd, String> {
    nix::fcntl::open(
        project_dir,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| errno_err("could not open the project directory", e))
}

pub(super) fn open_dir(parent: &impl AsFd, name: &str) -> Result<OwnedFd, String> {
    openat(parent, name, DIR_FLAGS, Mode::empty()).map_err(|e| errno_err("could not open a directory", e))
}

/// A regular file read in full, up to a cap.
pub(super) enum Read_ {
    Absent,
    /// A symbolic link or anything that is not a regular file.
    Foreign,
    TooLarge,
    Data {
        bytes: Vec<u8>,
        mode: u32,
    },
}

/// Reads `name` in `parent`: never follows a link, never blocks on a FIFO, checks the type on the
/// opened handle, and reads at most `cap` bytes.
pub(super) fn read_regular(parent: &impl AsFd, name: &str, cap: u64) -> Result<Read_, String> {
    let fd = match openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::ENOENT) => return Ok(Read_::Absent),
        Err(Errno::ELOOP | Errno::ENXIO | Errno::EISDIR | Errno::ENOTDIR | Errno::EACCES | Errno::EPERM) => {
            return Ok(Read_::Foreign);
        }
        Err(errno) => return Err(errno_err("could not open a file", errno)),
    };
    let stat = fstat(&fd).map_err(|e| errno_err("could not inspect a file", e))?;
    let kind = SFlag::from_bits_truncate(stat.st_mode as nix::libc::mode_t) & SFlag::S_IFMT;
    if kind != SFlag::S_IFREG {
        return Ok(Read_::Foreign);
    }
    if u64::try_from(stat.st_size).map_or(true, |size| size > cap) {
        return Ok(Read_::TooLarge);
    }
    let mut bytes = Vec::new();
    fs::File::from(fd)
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err("could not read a file", &e))?;
    if bytes.len() as u64 > cap {
        return Ok(Read_::TooLarge);
    }
    Ok(Read_::Data {
        bytes,
        // `st_mode` is `u16` on macOS and `u32` on Linux; `u32::from` is lossless on both and an
        // identity conversion on Linux only.
        #[allow(clippy::useless_conversion)]
        mode: u32::from(stat.st_mode) & 0o777,
    })
}

fn mode_of(mode: u32) -> Mode {
    // Only the permission bits: set-user-id, set-group-id and sticky are never carried over.
    Mode::from_bits_truncate(nix::libc::mode_t::try_from(mode & 0o777).unwrap_or(0o644))
}

/// Creates `name` in `parent` exclusively (never through a link, never over an existing file) with
/// exactly `mode` and writes `data`.
pub(super) fn create_file(parent: &impl AsFd, name: &str, data: &[u8], mode: u32) -> Result<(), String> {
    let fd = openat(
        parent,
        name,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        mode_of(mode),
    )
    .map_err(|e| errno_err("could not create a file", e))?;
    nix::sys::stat::fchmod(&fd, mode_of(mode)).map_err(|e| errno_err("could not set a file mode", e))?;
    let mut file = fs::File::from(fd);
    file.write_all(data).map_err(|e| io_err("could not write a file", &e))?;
    file.sync_all().map_err(|e| io_err("could not write a file", &e))
}

/// Replaces `name` in `parent` with `data` through an exclusively created temporary file and a
/// rename, with the given fixed mode.
pub(super) fn replace_file(parent: &impl AsFd, name: &str, data: &[u8], mode: u32) -> Result<(), String> {
    let temp = format!(".{name}.sylvode-tmp-{}", uuid::Uuid::new_v4().simple());
    create_file(parent, &temp, data, mode)?;
    renameat(parent, temp.as_str(), parent, name).map_err(|e| {
        let _ = unlinkat(parent, temp.as_str(), UnlinkatFlags::NoRemoveDir);
        errno_err("could not replace a file", e)
    })
}

/// Names in a directory, at most `limit` (more means the directory is not ours: `None`).
fn list_names(dir: &OwnedFd, limit: usize) -> Option<Vec<String>> {
    let mut handle = nix::dir::Dir::from_fd(dir.try_clone().ok()?).ok()?;
    let mut names = Vec::new();
    for entry in handle.iter() {
        let entry = entry.ok()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }
        names.push(name);
        if names.len() > limit {
            return None;
        }
    }
    Some(names)
}

/// Removes `name` in `parent`, directories recursively, without following links.
fn remove_tree(parent: &impl AsFd, name: &str, depth: usize) {
    if depth > 16 {
        return;
    }
    if matches!(classify(parent, name), Ok(Entry::Dir)) {
        if let Ok(dir) = open_dir(parent, name)
            && let Some(names) = list_names(&dir, usize::MAX - 1)
        {
            for child in names {
                remove_tree(&dir, &child, depth + 1);
            }
        }
        let _ = unlinkat(parent, name, UnlinkatFlags::RemoveDir);
    } else {
        let _ = unlinkat(parent, name, UnlinkatFlags::NoRemoveDir);
    }
}

// ----- backups ---------------------------------------------------------------------------------

fn private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| io_err("could not create the backup directory", &e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| io_err("could not restrict the backup directory", &e))?;
    }
    Ok(())
}

/// Writes a backup file that never overwrites an earlier one: `<stem>.<label>[.n]<ext>`.
fn backup_file(dir: &Path, stem: &str, label: &str, ext: &str, data: &[u8]) -> Result<(), String> {
    private_dir(dir)?;
    for attempt in 0..1000 {
        let name = if attempt == 0 {
            format!("{stem}.{label}{ext}")
        } else {
            format!("{stem}.{label}.{attempt}{ext}")
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(dir.join(&name)) {
            Ok(mut file) => {
                file.write_all(data)
                    .map_err(|e| io_err("could not write the backup", &e))?;
                prune(dir, stem);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_err("could not write the backup", &e)),
        }
    }
    Err("too many backups with the same label".to_string())
}

/// Keeps the newest [`MAX_BACKUPS`] entries whose name starts with `<stem>.`.
fn prune(dir: &Path, stem: &str) {
    let prefix = format!("{stem}.");
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    entries.sort();
    let excess = entries.len().saturating_sub(MAX_BACKUPS);
    for (_, path) in entries.into_iter().take(excess) {
        let _ = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
    }
}

struct BackupBudget {
    entries: usize,
    bytes: u64,
    complete: bool,
}

/// Copies the regular files of a skill directory into `dest` within the caps; links and special
/// files are skipped, and anything beyond a cap makes the backup incomplete.
fn copy_tree(src: &OwnedFd, dest: &Path, depth: usize, budget: &mut BackupBudget) -> Result<(), String> {
    private_dir(dest)?;
    let Some(names) = list_names(src, BACKUP_MAX_ENTRIES) else {
        budget.complete = false;
        return Ok(());
    };
    for name in names {
        budget.entries += 1;
        if budget.entries > BACKUP_MAX_ENTRIES {
            budget.complete = false;
            return Ok(());
        }
        match classify(src, &name)? {
            Entry::Dir if depth < BACKUP_MAX_DEPTH => {
                let child = open_dir(src, &name)?;
                copy_tree(&child, &dest.join(&name), depth + 1, budget)?;
            }
            Entry::File => match read_regular(src, &name, BACKUP_MAX_FILE_BYTES)? {
                Read_::Data { bytes, .. } => {
                    budget.bytes += bytes.len() as u64;
                    if budget.bytes > BACKUP_MAX_TOTAL_BYTES {
                        budget.complete = false;
                        return Ok(());
                    }
                    let mut options = fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let mut file = options
                        .open(dest.join(&name))
                        .map_err(|e| io_err("could not write the backup", &e))?;
                    file.write_all(&bytes)
                        .map_err(|e| io_err("could not write the backup", &e))?;
                }
                Read_::TooLarge => budget.complete = false,
                Read_::Absent | Read_::Foreign => {}
            },
            Entry::Dir | Entry::Other | Entry::Absent => {}
        }
    }
    Ok(())
}

// ----- the AGENTS.md managed section -----------------------------------------------------------

fn find_up_to(haystack: &[u8], needle: &[u8], limit: usize) -> Vec<usize> {
    let mut found = Vec::new();
    if needle.is_empty() || haystack.len() < needle.len() {
        return found;
    }
    for (at, window) in haystack.windows(needle.len()).enumerate() {
        if window == needle {
            found.push(at);
            if found.len() >= limit {
                break;
            }
        }
    }
    found
}

/// The line ending of a file: CRLF when it has at least one CRLF and at least half of its line
/// feeds are part of one, else LF. Counts without allocating.
fn line_ending(bytes: &[u8]) -> &'static str {
    let (mut crlf, mut lf, mut previous) = (0_usize, 0_usize, 0_u8);
    for byte in bytes.iter().copied() {
        if byte == b'\n' {
            lf += 1;
            if previous == b'\r' {
                crlf += 1;
            }
        }
        previous = byte;
    }
    if crlf > 0 && crlf * 2 >= lf { "\r\n" } else { "\n" }
}

fn normalise_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter().copied().peekable();
    while let Some(byte) = iter.next() {
        if byte == b'\r' && iter.peek() == Some(&b'\n') {
            continue;
        }
        out.push(byte);
    }
    out
}

fn with_ending(text: &str, ending: &str) -> String {
    let normalised = text.replace("\r\n", "\n");
    if ending == "\n" {
        normalised
    } else {
        normalised.replace('\n', ending)
    }
}

fn block(version: &str, body: &str, ending: &str) -> String {
    let body = body.replace("\r\n", "\n");
    let body = body.trim_end_matches('\n');
    let stamp = stamp(version, body.as_bytes());
    let text = format!("{BEGIN_PREFIX} {stamp}{BEGIN_SUFFIX}\n{body}\n{END_MARKER}");
    with_ending(&text, ending)
}

/// Installs or refreshes the managed section of `<project_dir>/AGENTS.md`.
///
/// - Exactly one BEGIN and one END, in order: the text between them (markers included) is
///   replaced, but only when it differs from the block that would be written; the previous block
///   is first saved as `<backup_dir>/AGENTS.section.<label>.md` (a numeric suffix keeps earlier
///   backups).
/// - No marker at all: the block is appended after a blank line, or the file is created holding
///   only the block.
/// - Any other count or order, or a file over [`AGENTS_MAX_BYTES`]: nothing is written
///   ([`Outcome::Conflict`]).
///
/// Bytes outside the markers are never changed. The block uses the line ending of the existing
/// file. A symbolic link or a file that is not a regular file is not touched
/// ([`Outcome::NotManaged`]).
///
/// # Errors
///
/// A short message (never file contents) when a file cannot be read or written.
pub fn install_agents_section(
    project_dir: &Path,
    content: &GuideContent<'_>,
    version: &str,
    backup_dir: &Path,
    backup_label: &str,
) -> Result<Outcome, String> {
    if !valid_label(backup_label) {
        return Err("the backup label must be letters, digits, '-' or '_'".to_string());
    }
    let root = open_root(project_dir)?;
    let (existing, mode) = match read_regular(&root, "AGENTS.md", AGENTS_MAX_BYTES)? {
        Read_::Foreign => return Ok(Outcome::NotManaged),
        Read_::TooLarge => return Ok(Outcome::Conflict),
        Read_::Absent => {
            let text = format!("{}\n", block(version, content.agents_section, "\n"));
            create_file(&root, "AGENTS.md", text.as_bytes(), 0o644)?;
            return Ok(Outcome::Created);
        }
        Read_::Data { bytes, mode } => (bytes, mode),
    };
    let ending = line_ending(&existing);
    let new_block = block(version, content.agents_section, ending);
    let begins = find_up_to(&existing, BEGIN_PREFIX.as_bytes(), 2);
    let ends = find_up_to(&existing, END_MARKER.as_bytes(), 2);
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut out = existing;
            out.extend_from_slice(ending.repeat(2).as_bytes());
            out.extend_from_slice(new_block.as_bytes());
            out.extend_from_slice(ending.as_bytes());
            replace_file(&root, "AGENTS.md", &out, mode)?;
            Ok(Outcome::Created)
        }
        ([begin], [end]) if begin < end => {
            let end_at = end + END_MARKER.len();
            let old = existing
                .get(*begin..end_at)
                .ok_or("marker positions are inconsistent")?;
            if normalise_crlf(old) == normalise_crlf(new_block.as_bytes()) {
                return Ok(Outcome::Unchanged);
            }
            backup_file(backup_dir, "AGENTS.section", backup_label, ".md", old)?;
            let mut out = existing.get(..*begin).unwrap_or_default().to_vec();
            out.extend_from_slice(new_block.as_bytes());
            out.extend_from_slice(existing.get(end_at..).unwrap_or_default());
            replace_file(&root, "AGENTS.md", &out, mode)?;
            Ok(Outcome::Replaced)
        }
        _ => Ok(Outcome::Conflict),
    }
}

// ----- the claude skill ------------------------------------------------------------------------

/// What the skill directory must hold: `SKILL.md` with its stamp comment, `reference/tools.md`,
/// and the marker with the stamp.
struct SkillFiles {
    skill_md: String,
    tools_md: String,
    marker: String,
}

fn skill_files(content: &GuideContent<'_>, version: &str) -> SkillFiles {
    let mut hashed = content.skill_md.as_bytes().to_vec();
    hashed.extend_from_slice(content.tools_md.as_bytes());
    let stamp = stamp(version, &hashed);
    let comment = format!("<!-- sylvode-skill {stamp} -->");
    SkillFiles {
        skill_md: insert_stamp_comment(content.skill_md, &comment),
        tools_md: content.tools_md.to_string(),
        marker: format!("{stamp}\n"),
    }
}

/// Puts the stamp comment on the line right after the frontmatter (or first when there is none).
fn insert_stamp_comment(skill_md: &str, comment: &str) -> String {
    let text = skill_md.replace("\r\n", "\n");
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(close) = rest.find("\n---\n")
    {
        let end = 4 + close + 5;
        let (front, after) = text.split_at(end);
        return format!("{front}{comment}\n{after}");
    }
    format!("{comment}\n{text}")
}

fn file_equals(parent: &impl AsFd, name: &str, expected: &str) -> bool {
    matches!(
        read_regular(parent, name, SKILL_FILE_MAX_BYTES),
        Ok(Read_::Data { bytes, .. }) if bytes == expected.as_bytes()
    )
}

/// Whether the skill directory holds exactly the expected three files and nothing else.
fn skill_matches(target: &OwnedFd, files: &SkillFiles) -> bool {
    let sorted = |names: Option<Vec<String>>| {
        names.map(|mut names| {
            names.sort();
            names
        })
    };
    if sorted(list_names(target, 8))
        != Some(vec![
            ".sylvode-managed".to_string(),
            "SKILL.md".to_string(),
            "reference".to_string(),
        ])
    {
        return false;
    }
    let Ok(Entry::Dir) = classify(target, "reference") else {
        return false;
    };
    let Ok(reference) = open_dir(target, "reference") else {
        return false;
    };
    sorted(list_names(&reference, 8)) == Some(vec!["tools.md".to_string()])
        && file_equals(target, ".sylvode-managed", &files.marker)
        && file_equals(target, "SKILL.md", &files.skill_md)
        && file_equals(&reference, "tools.md", &files.tools_md)
}

/// Opens `name` in `parent` as a directory, creating it when absent. `None` when something that is
/// not a plain directory (a symbolic link, a file) is in the way.
fn open_or_create_dir(parent: &impl AsFd, name: &str) -> Result<Option<OwnedFd>, String> {
    match classify(parent, name)? {
        Entry::Dir => open_dir(parent, name).map(Some),
        Entry::Absent => {
            mkdirat(parent, name, Mode::from_bits_truncate(0o755))
                .map_err(|e| errno_err("could not create a directory", e))?;
            open_dir(parent, name).map(Some)
        }
        Entry::File | Entry::Other => Ok(None),
    }
}

/// Installs or refreshes `<project>/.claude/skills/sylvode-mcp/`.
///
/// - The directory exists without `.sylvode-managed`, is a symbolic link, or a link or file is in
///   the way anywhere on the path: untouched ([`Outcome::NotManaged`]).
/// - The directory holds exactly the three expected files with exactly the expected bytes:
///   nothing is written ([`Outcome::Unchanged`]). A forged stamp over other content, or an extra
///   entry, is not "unchanged".
/// - Otherwise the directory is backed up to `<backup_dir>/skill.<label>/` (within the backup
///   caps), the three files are written to a new directory in the same parent, and that directory
///   is renamed into place (the old directory is renamed aside first, then removed).
///
/// # Errors
///
/// A short message when a file cannot be read or written.
pub fn install_claude_skill(
    project_dir: &Path,
    content: &GuideContent<'_>,
    version: &str,
    backup_dir: &Path,
    backup_label: &str,
) -> Result<Outcome, String> {
    if !valid_label(backup_label) {
        return Err("the backup label must be letters, digits, '-' or '_'".to_string());
    }
    let files = skill_files(content, version);
    let root = open_root(project_dir)?;
    let Some(claude) = open_or_create_dir(&root, ".claude")? else {
        return Ok(Outcome::NotManaged);
    };
    let Some(skills) = open_or_create_dir(&claude, "skills")? else {
        return Ok(Outcome::NotManaged);
    };
    let existing = match classify(&skills, "sylvode-mcp")? {
        Entry::Absent => None,
        Entry::Dir => Some(open_dir(&skills, "sylvode-mcp")?),
        Entry::File | Entry::Other => return Ok(Outcome::NotManaged),
    };
    let mut backup_complete = true;
    if let Some(target) = &existing {
        match read_regular(target, ".sylvode-managed", MARKER_MAX_BYTES)? {
            Read_::Data { .. } => {}
            Read_::Absent | Read_::Foreign | Read_::TooLarge => return Ok(Outcome::NotManaged),
        }
        if skill_matches(target, &files) {
            return Ok(Outcome::Unchanged);
        }
        let mut budget = BackupBudget {
            entries: 0,
            bytes: 0,
            complete: true,
        };
        private_dir(backup_dir)?;
        let mut dest = backup_dir.join(format!("skill.{backup_label}"));
        let mut attempt = 0;
        while dest.exists() {
            attempt += 1;
            dest = backup_dir.join(format!("skill.{backup_label}.{attempt}"));
        }
        copy_tree(target, &dest, 0, &mut budget)?;
        backup_complete = budget.complete;
        prune(backup_dir, "skill");
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let staging_name = format!(".sylvode-mcp.new-{suffix}");
    mkdirat(&skills, staging_name.as_str(), Mode::from_bits_truncate(0o755))
        .map_err(|e| errno_err("could not create a directory", e))?;
    let write_all = || -> Result<(), String> {
        let staging = open_dir(&skills, &staging_name)?;
        mkdirat(&staging, "reference", Mode::from_bits_truncate(0o755))
            .map_err(|e| errno_err("could not create a directory", e))?;
        let reference = open_dir(&staging, "reference")?;
        create_file(&staging, "SKILL.md", files.skill_md.as_bytes(), 0o644)?;
        create_file(&reference, "tools.md", files.tools_md.as_bytes(), 0o644)?;
        create_file(&staging, ".sylvode-managed", files.marker.as_bytes(), 0o644)
    };
    if let Err(error) = write_all() {
        remove_tree(&skills, &staging_name, 0);
        return Err(error);
    }
    if existing.is_some() {
        let aside = format!(".sylvode-mcp.old-{suffix}");
        renameat(&skills, "sylvode-mcp", &skills, aside.as_str())
            .map_err(|e| errno_err("could not move the old skill aside", e))?;
        if let Err(e) = renameat(&skills, staging_name.as_str(), &skills, "sylvode-mcp") {
            let _ = renameat(&skills, aside.as_str(), &skills, "sylvode-mcp");
            remove_tree(&skills, &staging_name, 0);
            return Err(errno_err("could not put the skill in place", e));
        }
        remove_tree(&skills, &aside, 0);
        Ok(if backup_complete {
            Outcome::Replaced
        } else {
            Outcome::ReplacedBackupIncomplete
        })
    } else {
        renameat(&skills, staging_name.as_str(), &skills, "sylvode-mcp").map_err(|e| {
            remove_tree(&skills, &staging_name, 0);
            errno_err("could not put the skill in place", e)
        })?;
        Ok(Outcome::Created)
    }
}

// ----- git exclude -----------------------------------------------------------------------------

/// Adds [`EXCLUDE_LINE`] to `<project>/.git/info/exclude`, once.
///
/// Only when `.git` is a real directory. A `.git` file (a worktree) is reported as
/// [`ExcludeOutcome::Worktree`] and not written: its exclude file belongs to the main repository.
/// A symbolic link at `.git`, `.git/info` or `.git/info/exclude`, a non-regular or oversized
/// exclude file: [`ExcludeOutcome::Refused`].
///
/// # Errors
///
/// A short message when the file cannot be written.
pub fn ensure_git_exclude(project_dir: &Path) -> Result<ExcludeOutcome, String> {
    let root = open_root(project_dir)?;
    match classify(&root, ".git")? {
        Entry::Absent => return Ok(ExcludeOutcome::NoGit),
        Entry::File => return Ok(ExcludeOutcome::Worktree),
        Entry::Other => return Ok(ExcludeOutcome::Refused),
        Entry::Dir => {}
    }
    let git = open_dir(&root, ".git")?;
    let Some(info) = open_or_create_dir(&git, "info")? else {
        return Ok(ExcludeOutcome::Refused);
    };
    let (mut text, mode) = match read_regular(&info, "exclude", EXCLUDE_MAX_BYTES)? {
        Read_::Absent => (Vec::new(), 0o644),
        Read_::Data { bytes, mode } => (bytes, mode),
        Read_::Foreign | Read_::TooLarge => return Ok(ExcludeOutcome::Refused),
    };
    if text
        .split(|b| *b == b'\n')
        .any(|line| line.trim_ascii() == EXCLUDE_LINE.as_bytes())
    {
        return Ok(ExcludeOutcome::Present);
    }
    if !text.is_empty() && text.last() != Some(&b'\n') {
        text.push(b'\n');
    }
    text.extend_from_slice(EXCLUDE_LINE.as_bytes());
    text.push(b'\n');
    replace_file(&info, "exclude", &text, mode)?;
    Ok(ExcludeOutcome::Added)
}
