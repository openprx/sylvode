//! Installing the agent guide into a project (design sections 4.10 and 8.5).
//!
//! - **codex and opencode:** a managed section in the project's `AGENTS.md`.
//! - **claude:** the skill directory `.claude/skills/sylvode-mcp/` with `SKILL.md`,
//!   `reference/tools.md` and `.sylvode-managed`, plus a `.git/info/exclude` line.
//!
//! Nothing outside the markers of the managed section is ever changed, a skill directory without
//! `.sylvode-managed` is never touched, and every replacement keeps a backup first. The content
//! comes in through [`GuideContent`]; the generated files of the skills source of truth
//! (`skills/sylvode-mcp`, work package P3.6) are not in the tree yet, so no embedded constructor
//! exists here. It is added where the generator's output lands, and until then callers pass the
//! content explicitly.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const BEGIN_PREFIX: &str = "<!-- BEGIN SYLVODE MANAGED SECTION";
const END_MARKER: &str = "<!-- END SYLVODE MANAGED SECTION -->";
const BEGIN_SUFFIX: &str = ". Do not edit: sylvode-runner replaces this block. -->";

/// The line added to `.git/info/exclude` for the claude skill directory.
pub const EXCLUDE_LINE: &str = ".claude/skills/sylvode-mcp/";

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
    /// The file or directory was created.
    Created,
    /// A different stamp was replaced, after a backup.
    Replaced,
    /// The stamp was equal; nothing was written.
    Unchanged,
    /// The marker lines are not exactly one BEGIN followed by one END; nothing was written. The
    /// task still runs, because the server instructions carry the procedure too. `doctor` reports
    /// `agent_guide: conflict`.
    Conflict,
    /// A skill directory exists without `.sylvode-managed`, or the target is a symbolic link;
    /// it is not ours and was not touched.
    NotManaged,
}

fn io_err(what: &str, error: &std::io::Error) -> String {
    format!("{what}: {}", error.kind())
}

fn hex12(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest).chars().take(12).collect()
}

/// `v=<version> h=<first 12 hex of the SHA-256 of what was installed>`.
#[must_use]
pub fn stamp(version: &str, hashed: &[u8]) -> String {
    format!("v={version} h={}", hex12(hashed))
}

fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.len() <= 64 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn backup_file(dir: &Path, name: &str, data: &[u8]) -> Result<(), String> {
    create_private_dir(dir)?;
    fs::write(dir.join(name), data).map_err(|e| io_err("could not write the backup", &e))
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| io_err("could not create a directory", &e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(at, window)| (window == needle).then_some(at))
        .collect()
}

/// The line ending of a file: CRLF when it has at least as many CRLF as bare LF line ends and at
/// least one, else LF.
fn line_ending(bytes: &[u8]) -> &'static str {
    let crlf = find_all(bytes, b"\r\n").len();
    let lf = find_all(bytes, b"\n").len();
    if crlf > 0 && crlf * 2 >= lf { "\r\n" } else { "\n" }
}

fn with_ending(text: &str, ending: &str) -> String {
    let normalised = text.replace("\r\n", "\n");
    if ending == "\n" {
        normalised
    } else {
        normalised.replace('\n', ending)
    }
}

fn block(version: &str, body: &str, ending: &str) -> (String, String) {
    let body = body.replace("\r\n", "\n");
    let body = body.trim_end_matches('\n');
    let stamp = stamp(version, body.as_bytes());
    let text = format!("{BEGIN_PREFIX} {stamp}{BEGIN_SUFFIX}\n{body}\n{END_MARKER}");
    (with_ending(&text, ending), stamp)
}

/// Writes `data` to `path` through a temporary file in the same directory and a rename, keeping
/// the permissions of the file it replaces.
fn write_atomically(path: &Path, data: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("the path has no parent directory")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("the path has no file name")?;
    let temp = parent.join(format!(".{name}.sylvode-tmp-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| io_err("could not create a temporary file", &e))?;
        file.write_all(data)
            .map_err(|e| io_err("could not write the file", &e))?;
        file.sync_all().map_err(|e| io_err("could not write the file", &e))?;
        if let Ok(metadata) = fs::metadata(path) {
            let _ = fs::set_permissions(&temp, metadata.permissions());
        }
        fs::rename(&temp, path).map_err(|e| io_err("could not replace the file", &e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Installs or refreshes the managed section of `agents_md` (usually `<project>/AGENTS.md`).
///
/// - Exactly one BEGIN and one END, in order: the text between them (markers included) is
///   replaced, but only when the stamp differs; the previous block is first saved as
///   `<backup_dir>/AGENTS.section.<label>.md`.
/// - No marker at all: the block is appended after a blank line, or the file is created holding
///   only the block.
/// - Any other count or order: nothing is written ([`Outcome::Conflict`]).
///
/// Bytes outside the markers are never changed. The block uses the line ending of the existing
/// file. A symbolic link is not followed or replaced ([`Outcome::NotManaged`]).
///
/// # Errors
///
/// A short message (never file contents) when a file cannot be read or written.
pub fn install_agents_section(
    agents_md: &Path,
    content: &GuideContent<'_>,
    version: &str,
    backup_dir: &Path,
    backup_label: &str,
) -> Result<Outcome, String> {
    if !valid_label(backup_label) {
        return Err("the backup label must be letters, digits, '-' or '_'".to_string());
    }
    if is_symlink(agents_md) {
        return Ok(Outcome::NotManaged);
    }
    let existing = match fs::read(agents_md) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err("could not read AGENTS.md", &e)),
    };
    let Some(existing) = existing else {
        let (text, _) = block(version, content.agents_section, "\n");
        write_atomically(agents_md, format!("{text}\n").as_bytes())?;
        return Ok(Outcome::Created);
    };
    let ending = line_ending(&existing);
    let (new_block, new_stamp) = block(version, content.agents_section, ending);
    let begins = find_all(&existing, BEGIN_PREFIX.as_bytes());
    let ends = find_all(&existing, END_MARKER.as_bytes());
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut out = existing;
            out.extend_from_slice(ending.repeat(2).as_bytes());
            out.extend_from_slice(new_block.as_bytes());
            out.extend_from_slice(ending.as_bytes());
            write_atomically(agents_md, &out)?;
            Ok(Outcome::Created)
        }
        ([begin], [end]) if begin < end => {
            let end_at = end + END_MARKER.len();
            let old = existing
                .get(*begin..end_at)
                .ok_or("marker positions are inconsistent")?;
            let first_line_end = old.iter().position(|b| *b == b'\n').unwrap_or(old.len());
            let first_line = String::from_utf8_lossy(old.get(..first_line_end).unwrap_or_default());
            if first_line.contains(&format!("{BEGIN_PREFIX} {new_stamp}{BEGIN_SUFFIX}")) {
                return Ok(Outcome::Unchanged);
            }
            backup_file(backup_dir, &format!("AGENTS.section.{backup_label}.md"), old)?;
            let mut out = existing.get(..*begin).unwrap_or_default().to_vec();
            out.extend_from_slice(new_block.as_bytes());
            out.extend_from_slice(existing.get(end_at..).unwrap_or_default());
            write_atomically(agents_md, &out)?;
            Ok(Outcome::Replaced)
        }
        _ => Ok(Outcome::Conflict),
    }
}

fn skill_files(content: &GuideContent<'_>, version: &str) -> (String, Vec<u8>, String) {
    let mut hashed = content.skill_md.as_bytes().to_vec();
    hashed.extend_from_slice(content.tools_md.as_bytes());
    let stamp = stamp(version, &hashed);
    let comment = format!("<!-- sylvode-skill {stamp} -->");
    (insert_stamp_comment(content.skill_md, &comment), hashed, stamp)
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

/// Installs or refreshes `<project>/.claude/skills/sylvode-mcp/`.
///
/// - The directory exists without `.sylvode-managed`, or is a symbolic link: untouched
///   ([`Outcome::NotManaged`]).
/// - The stamp in `.sylvode-managed` is equal: no write.
/// - Otherwise the directory is backed up to `<backup_dir>/skill.<label>/`, the three files are
///   written to a temporary directory in the same parent, and that directory is renamed into
///   place (the old directory is renamed aside first, then removed).
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
    let claude = project_dir.join(".claude");
    let skills = claude.join("skills");
    let target = skills.join("sylvode-mcp");
    if [&claude, &skills, &target].iter().any(|path| is_symlink(path)) {
        return Ok(Outcome::NotManaged);
    }
    let (skill_md, _, stamp) = skill_files(content, version);
    let existed = target.is_dir();
    if existed {
        let marker = fs::read_to_string(target.join(".sylvode-managed")).ok();
        let Some(marker) = marker else {
            return Ok(Outcome::NotManaged);
        };
        if marker.trim() == stamp {
            return Ok(Outcome::Unchanged);
        }
        copy_dir(&target, &backup_dir.join(format!("skill.{backup_label}")))?;
    } else if target.exists() {
        return Ok(Outcome::NotManaged);
    }
    fs::create_dir_all(&skills).map_err(|e| io_err("could not create the skills directory", &e))?;
    let suffix = uuid::Uuid::new_v4().simple();
    let staging = skills.join(format!(".sylvode-mcp.new-{suffix}"));
    let write_all = || -> Result<(), String> {
        fs::create_dir_all(staging.join("reference")).map_err(|e| io_err("could not create a directory", &e))?;
        fs::write(staging.join("SKILL.md"), skill_md.as_bytes()).map_err(|e| io_err("could not write SKILL.md", &e))?;
        fs::write(staging.join("reference").join("tools.md"), content.tools_md.as_bytes())
            .map_err(|e| io_err("could not write tools.md", &e))?;
        fs::write(staging.join(".sylvode-managed"), format!("{stamp}\n"))
            .map_err(|e| io_err("could not write .sylvode-managed", &e))
    };
    if let Err(error) = write_all() {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if existed {
        let aside = skills.join(format!(".sylvode-mcp.old-{suffix}"));
        fs::rename(&target, &aside).map_err(|e| io_err("could not move the old skill aside", &e))?;
        if let Err(e) = fs::rename(&staging, &target) {
            let _ = fs::rename(&aside, &target);
            let _ = fs::remove_dir_all(&staging);
            return Err(io_err("could not put the skill in place", &e));
        }
        let _ = fs::remove_dir_all(&aside);
        Ok(Outcome::Replaced)
    } else {
        fs::rename(&staging, &target).map_err(|e| {
            let _ = fs::remove_dir_all(&staging);
            io_err("could not put the skill in place", &e)
        })?;
        Ok(Outcome::Created)
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    create_private_dir(to)?;
    for entry in fs::read_dir(from).map_err(|e| io_err("could not read the skill directory", &e))? {
        let entry = entry.map_err(|e| io_err("could not read the skill directory", &e))?;
        let file_type = entry
            .file_type()
            .map_err(|e| io_err("could not read the skill directory", &e))?;
        let dest: PathBuf = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &dest).map_err(|e| io_err("could not copy a file", &e))?;
        }
    }
    Ok(())
}

/// Adds [`EXCLUDE_LINE`] to `<project>/.git/info/exclude`, once. Only when `.git` is a directory:
/// a `.git` file (a worktree) or no `.git` is left alone. Returns whether the line was added.
///
/// # Errors
///
/// A short message when the file cannot be read or written.
pub fn ensure_git_exclude(project_dir: &Path) -> Result<bool, String> {
    let git = project_dir.join(".git");
    if is_symlink(&git) || !git.is_dir() {
        return Ok(false);
    }
    let info = git.join("info");
    let exclude = info.join("exclude");
    let existing = match fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_err("could not read the git exclude file", &e)),
    };
    if existing.lines().any(|line| line.trim() == EXCLUDE_LINE) {
        return Ok(false);
    }
    fs::create_dir_all(&info).map_err(|e| io_err("could not create .git/info", &e))?;
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(EXCLUDE_LINE);
    text.push('\n');
    write_atomically(&exclude, text.as_bytes())?;
    Ok(true)
}
