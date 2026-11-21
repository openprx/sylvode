use std::fs;
use std::path::{Path, PathBuf};

use super::guide::{
    EXCLUDE_LINE, GuideContent, Outcome, ensure_git_exclude, install_agents_section, install_claude_skill, stamp,
};

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-guide-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const BODY: &str = "## Sylvode tasks\nUse the task tools.\n";
const VERSION: &str = "0.5.0";

fn content() -> GuideContent<'static> {
    GuideContent {
        skill_md: "---\nname: sylvode-mcp\ndescription: Use Sylvode.\n---\n# Skill\nText.\n",
        tools_md: "# Tools\n- one\n",
        agents_section: BODY,
    }
}

fn begin_line() -> String {
    format!(
        "<!-- BEGIN SYLVODE MANAGED SECTION {}. Do not edit: sylvode-runner replaces this block. -->",
        stamp(VERSION, BODY.trim_end_matches('\n').as_bytes())
    )
}

fn block() -> String {
    format!(
        "{}\n{}\n<!-- END SYLVODE MANAGED SECTION -->",
        begin_line(),
        BODY.trim_end()
    )
}

fn install(file: &Path, backups: &Path) -> Result<Outcome, String> {
    install_agents_section(file, &content(), VERSION, backups, "t1")
}

#[test]
fn a_missing_file_is_created_with_only_the_section() {
    let dir = Dir::new();
    let file = dir.path().join("AGENTS.md");
    assert_eq!(install(&file, &dir.path().join("b")), Ok(Outcome::Created));
    assert_eq!(fs::read_to_string(&file).expect("read"), format!("{}\n", block()));
    assert!(begin_line().contains("v=0.5.0 h="));
    // The stamp's hash is 12 hex characters.
    let hash = begin_line()
        .split(" h=")
        .nth(1)
        .expect("hash")
        .chars()
        .take(12)
        .collect::<String>();
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
}

#[test]
fn zero_markers_append_after_a_blank_line_and_keep_the_file() {
    let dir = Dir::new();
    let file = dir.path().join("AGENTS.md");
    fs::write(&file, "# Project\n\nExisting rules.\n").expect("write");
    assert_eq!(install(&file, &dir.path().join("b")), Ok(Outcome::Created));
    assert_eq!(
        fs::read_to_string(&file).expect("read"),
        format!("# Project\n\nExisting rules.\n\n\n{}\n", block())
    );
}

#[test]
fn one_pair_of_markers_is_replaced_only_when_the_stamp_differs() {
    let dir = Dir::new();
    let file = dir.path().join("AGENTS.md");
    let backups = dir.path().join("backups").join("KEY");
    let old = "<!-- BEGIN SYLVODE MANAGED SECTION v=0.4.0 h=aaaaaaaaaaaa. Do not edit: sylvode-runner replaces this block. -->\nold body\n<!-- END SYLVODE MANAGED SECTION -->";
    fs::write(&file, format!("before\n{old}\nafter\n")).expect("write");
    assert_eq!(install(&file, &backups), Ok(Outcome::Replaced));
    assert_eq!(
        fs::read_to_string(&file).expect("read"),
        format!("before\n{}\nafter\n", block())
    );
    assert_eq!(
        fs::read_to_string(backups.join("AGENTS.section.t1.md")).expect("backup"),
        old
    );
    // A second install finds an equal stamp and writes nothing.
    let before = fs::metadata(&file).expect("meta").modified().expect("mtime");
    assert_eq!(install(&file, &backups), Ok(Outcome::Unchanged));
    assert_eq!(fs::metadata(&file).expect("meta").modified().expect("mtime"), before);
    assert_eq!(fs::read_dir(&backups).expect("dir").count(), 1);
}

#[test]
fn crlf_files_keep_their_line_endings() {
    let dir = Dir::new();
    let file = dir.path().join("AGENTS.md");
    let old = "<!-- BEGIN SYLVODE MANAGED SECTION v=0.4.0 h=aaaaaaaaaaaa. Do not edit: sylvode-runner replaces this block. -->\r\nold\r\n<!-- END SYLVODE MANAGED SECTION -->";
    fs::write(&file, format!("# A\r\n\r\ntext\r\n{old}\r\ntail\r\n")).expect("write");
    assert_eq!(install(&file, &dir.path().join("b")), Ok(Outcome::Replaced));
    let want = format!("# A\r\n\r\ntext\r\n{}\r\ntail\r\n", block().replace('\n', "\r\n"));
    assert_eq!(fs::read(&file).expect("read"), want.as_bytes());
    // Appending to a CRLF file uses CRLF as well.
    let other = dir.path().join("OTHER.md");
    fs::write(&other, "one\r\ntwo\r\n").expect("write");
    assert_eq!(install(&other, &dir.path().join("b")), Ok(Outcome::Created));
    assert_eq!(
        fs::read(&other).expect("read"),
        format!("one\r\ntwo\r\n\r\n\r\n{}\r\n", block().replace('\n', "\r\n")).as_bytes()
    );
}

#[test]
fn any_other_marker_layout_is_a_conflict_and_nothing_is_written() {
    let dir = Dir::new();
    let begin = "<!-- BEGIN SYLVODE MANAGED SECTION v=0.4.0 h=aaaaaaaaaaaa. Do not edit: sylvode-runner replaces this block. -->";
    let end = "<!-- END SYLVODE MANAGED SECTION -->";
    for (name, text) in [
        ("begin_only", format!("x\n{begin}\nbody\n")),
        ("end_only", format!("x\n{end}\n")),
        ("two_pairs", format!("{begin}\na\n{end}\n{begin}\nb\n{end}\n")),
        ("reversed", format!("{end}\nbody\n{begin}\n")),
        ("two_begins", format!("{begin}\n{begin}\n{end}\n")),
    ] {
        let file = dir.path().join(format!("{name}.md"));
        fs::write(&file, &text).expect("write");
        assert_eq!(install(&file, &dir.path().join("b")), Ok(Outcome::Conflict), "{name}");
        assert_eq!(fs::read_to_string(&file).expect("read"), text, "{name}");
    }
    assert!(!dir.path().join("b").exists());
}

#[test]
fn a_symlinked_agents_file_is_not_followed() {
    let dir = Dir::new();
    let real = dir.path().join("real.md");
    fs::write(&real, "real\n").expect("write");
    let link = dir.path().join("AGENTS.md");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    assert_eq!(install(&link, &dir.path().join("b")), Ok(Outcome::NotManaged));
    assert_eq!(fs::read_to_string(&real).expect("read"), "real\n");
    assert!(fs::symlink_metadata(&link).expect("meta").file_type().is_symlink());
}

#[test]
fn backup_labels_cannot_escape_the_backup_directory() {
    let dir = Dir::new();
    let file = dir.path().join("AGENTS.md");
    assert!(install_agents_section(&file, &content(), VERSION, dir.path(), "../x").is_err());
    assert!(install_claude_skill(dir.path(), &content(), VERSION, dir.path(), "a/b").is_err());
    assert!(!file.exists());
}

fn skill_dir(project: &Path) -> PathBuf {
    project.join(".claude").join("skills").join("sylvode-mcp")
}

#[test]
fn the_claude_skill_is_created_with_a_stamp_after_the_frontmatter() {
    let dir = Dir::new();
    let backups = dir.path().join("backups");
    assert_eq!(
        install_claude_skill(dir.path(), &content(), VERSION, &backups, "t1"),
        Ok(Outcome::Created)
    );
    let target = skill_dir(dir.path());
    let marker = fs::read_to_string(target.join(".sylvode-managed")).expect("marker");
    let want_stamp = stamp(VERSION, [content().skill_md, content().tools_md].concat().as_bytes());
    assert_eq!(marker, format!("{want_stamp}\n"));
    let skill = fs::read_to_string(target.join("SKILL.md")).expect("skill");
    assert_eq!(
        skill,
        format!(
            "---\nname: sylvode-mcp\ndescription: Use Sylvode.\n---\n<!-- sylvode-skill {want_stamp} -->\n# Skill\nText.\n"
        )
    );
    assert_eq!(
        fs::read_to_string(target.join("reference").join("tools.md")).expect("tools"),
        "# Tools\n- one\n"
    );
    assert!(!backups.exists());
    // No temporary directory is left behind.
    let names: Vec<_> = fs::read_dir(dir.path().join(".claude").join("skills"))
        .expect("dir")
        .collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn an_equal_stamp_writes_nothing_and_a_different_one_backs_up_and_replaces() {
    let dir = Dir::new();
    let backups = dir.path().join("backups");
    install_claude_skill(dir.path(), &content(), VERSION, &backups, "t1").expect("install");
    let target = skill_dir(dir.path());
    let before = fs::metadata(target.join("SKILL.md"))
        .expect("meta")
        .modified()
        .expect("mtime");
    assert_eq!(
        install_claude_skill(dir.path(), &content(), VERSION, &backups, "t2"),
        Ok(Outcome::Unchanged)
    );
    assert_eq!(
        fs::metadata(target.join("SKILL.md"))
            .expect("meta")
            .modified()
            .expect("mtime"),
        before
    );
    assert!(!backups.exists());

    let newer = GuideContent {
        tools_md: "# Tools\n- one\n- two\n",
        ..content()
    };
    assert_eq!(
        install_claude_skill(dir.path(), &newer, "0.5.1", &backups, "t3"),
        Ok(Outcome::Replaced)
    );
    assert_eq!(
        fs::read_to_string(target.join("reference").join("tools.md")).expect("tools"),
        "# Tools\n- one\n- two\n"
    );
    assert!(
        fs::read_to_string(target.join(".sylvode-managed"))
            .expect("marker")
            .starts_with("v=0.5.1 h=")
    );
    let saved = backups.join("skill.t3");
    assert_eq!(
        fs::read_to_string(saved.join("reference").join("tools.md")).expect("backup"),
        "# Tools\n- one\n"
    );
    assert!(
        fs::read_to_string(saved.join(".sylvode-managed"))
            .expect("backup marker")
            .starts_with("v=0.5.0 h=")
    );
}

#[test]
fn a_skill_directory_without_the_marker_is_never_touched() {
    let dir = Dir::new();
    let target = skill_dir(dir.path());
    fs::create_dir_all(&target).expect("mkdir");
    fs::write(target.join("SKILL.md"), "mine").expect("write");
    let backups = dir.path().join("backups");
    assert_eq!(
        install_claude_skill(dir.path(), &content(), VERSION, &backups, "t1"),
        Ok(Outcome::NotManaged)
    );
    assert_eq!(fs::read_to_string(target.join("SKILL.md")).expect("read"), "mine");
    assert!(!target.join(".sylvode-managed").exists());
    assert!(!backups.exists());
}

#[test]
fn a_symlinked_skill_directory_is_not_followed() {
    let dir = Dir::new();
    let elsewhere = Dir::new();
    let skills = dir.path().join(".claude").join("skills");
    fs::create_dir_all(&skills).expect("mkdir");
    std::os::unix::fs::symlink(elsewhere.path(), skills.join("sylvode-mcp")).expect("symlink");
    assert_eq!(
        install_claude_skill(dir.path(), &content(), VERSION, &dir.path().join("b"), "t1"),
        Ok(Outcome::NotManaged)
    );
    assert_eq!(fs::read_dir(elsewhere.path()).expect("dir").count(), 0);
}

#[test]
fn skill_text_without_frontmatter_gets_the_stamp_first() {
    let dir = Dir::new();
    let plain = GuideContent {
        skill_md: "# Skill\n",
        ..content()
    };
    install_claude_skill(dir.path(), &plain, VERSION, &dir.path().join("b"), "t1").expect("install");
    let skill = fs::read_to_string(skill_dir(dir.path()).join("SKILL.md")).expect("skill");
    assert!(skill.starts_with("<!-- sylvode-skill v=0.5.0 h="), "{skill}");
    assert!(skill.ends_with("-->\n# Skill\n"), "{skill}");
}

#[test]
fn the_git_exclude_line_is_added_once() {
    let dir = Dir::new();
    // No .git: nothing to do.
    assert_eq!(ensure_git_exclude(dir.path()), Ok(false));
    fs::create_dir_all(dir.path().join(".git")).expect("mkdir");
    assert_eq!(ensure_git_exclude(dir.path()), Ok(true));
    assert_eq!(ensure_git_exclude(dir.path()), Ok(false));
    let text = fs::read_to_string(dir.path().join(".git").join("info").join("exclude")).expect("read");
    assert_eq!(text, format!("{EXCLUDE_LINE}\n"));
    // An existing file without a final newline keeps its lines.
    let other = Dir::new();
    let info = other.path().join(".git").join("info");
    fs::create_dir_all(&info).expect("mkdir");
    fs::write(info.join("exclude"), "# comment\n*.log").expect("write");
    assert_eq!(ensure_git_exclude(other.path()), Ok(true));
    assert_eq!(
        fs::read_to_string(info.join("exclude")).expect("read"),
        format!("# comment\n*.log\n{EXCLUDE_LINE}\n")
    );
}

#[test]
fn a_git_file_from_a_worktree_is_left_alone() {
    let dir = Dir::new();
    fs::write(dir.path().join(".git"), "gitdir: elsewhere\n").expect("write");
    assert_eq!(ensure_git_exclude(dir.path()), Ok(false));
    assert!(dir.path().join(".git").is_file());
}
