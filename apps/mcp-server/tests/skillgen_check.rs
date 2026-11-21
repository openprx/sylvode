//! The checked-in skill and every generated block follow the tool catalogue: `sylvode-skillgen
//! --check` regenerates in memory and finds no difference, and `--lint` finds no drift in the
//! hand-written text. The fix for a failure is `sylvode-skillgen --write`.

use std::process::{Command, Output};

fn skillgen(mode: &str) -> Result<Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_sylvode-skillgen")).arg(mode).output()
}

fn report(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_generated_files_are_current() -> Result<(), Box<dyn std::error::Error>> {
    let output = skillgen("--check")?;
    assert!(
        output.status.success(),
        "run `sylvode-skillgen --write`:\n{}",
        report(&output)
    );
    Ok(())
}

#[test]
fn the_hand_written_skill_text_lints_clean() -> Result<(), Box<dyn std::error::Error>> {
    let output = skillgen("--lint")?;
    assert!(output.status.success(), "{}", report(&output));
    Ok(())
}

#[test]
fn a_run_without_a_mode_or_with_two_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let none = Command::new(env!("CARGO_BIN_EXE_sylvode-skillgen")).output()?;
    assert_eq!(none.status.code(), Some(2));
    let two = Command::new(env!("CARGO_BIN_EXE_sylvode-skillgen"))
        .args(["--write", "--check"])
        .output()?;
    assert_eq!(two.status.code(), Some(2));
    Ok(())
}

#[test]
fn check_fails_with_a_diff_when_a_generated_file_is_stale() -> Result<(), Box<dyn std::error::Error>> {
    // A copy of the checked-in tree files the generator reads, with one summary edited by hand.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scratch = std::env::temp_dir().join(format!("sylvode-skillgen-stale-{}", std::process::id()));
    for relative in [
        "skills/sylvode-mcp/SKILL.md",
        "skills/sylvode-mcp/reference/tools.md",
        "skills/sylvode-mcp/generated/AGENTS.section.md",
        "skills/sylvode-mcp/generated/server-instructions.md",
        "skills/sylvode-mcp/generated/recipes.md",
        "apps/mcp-server/README.md",
        "apps/mcp-server/AGENTS.md",
        "README.md",
        "docs/README.md",
        "apps/api/tests/fixtures/mcp-tool-names.json",
    ] {
        let target = scratch.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(root.join(relative), &target)?;
    }
    let tools = scratch.join("skills/sylvode-mcp/reference/tools.md");
    let edited = std::fs::read_to_string(&tools)?.replace("List the projects of the bound workspace", "Edited by hand");
    std::fs::write(&tools, edited)?;
    let output = Command::new(env!("CARGO_BIN_EXE_sylvode-skillgen"))
        .args(["--check", "--root"])
        .arg(&scratch)
        .output()?;
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(output.status.code(), Some(1));
    let text = report(&output);
    assert!(
        text.contains("-- `projects.list`") || text.contains("-- a/skills/sylvode-mcp/reference/tools.md"),
        "{text}"
    );
    assert!(text.contains("sylvode-skillgen -- --write"), "{text}");
    Ok(())
}
