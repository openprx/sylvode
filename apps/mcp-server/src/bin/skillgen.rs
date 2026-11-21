// Generates, checks and lints `skills/sylvode-mcp` and every file that carries a generated block.
// Needs no database or network.
use mcp_server::skillgen::{Catalogue, LintInput, Output, Sources, lint, outputs, paths, unified_diff};
use mcp_server::tools::catalog::{CATALOG, CATEGORIES};
use mcp_server::tools::entry::ENTRY_TOOL_NAMES;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: sylvode-skillgen (--write | --check | --lint) [--root PATH]";
const FIX: &str = "cargo run -p mcp-server --bin sylvode-skillgen -- --write";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Write,
    Check,
    Lint,
}

struct Arguments {
    mode: Mode,
    root: PathBuf,
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut mode = None;
    let mut root = None;
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let chosen = match argument.as_str() {
            "--write" => Some(Mode::Write),
            "--check" => Some(Mode::Check),
            "--lint" => Some(Mode::Lint),
            "--root" => {
                let value = rest.next().ok_or("--root needs a path")?;
                root = Some(PathBuf::from(value));
                None
            }
            other => return Err(format!("unknown argument {other}")),
        };
        if let Some(chosen) = chosen {
            if mode.replace(chosen).is_some() {
                return Err("choose one of --write, --check and --lint".to_string());
            }
        }
    }
    Ok(Arguments {
        mode: mode.ok_or("choose one of --write, --check and --lint")?,
        root: root.unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))),
    })
}

fn read(root: &Path, relative: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(relative)).map_err(|error| format!("cannot read {relative}: {error}"))
}

fn catalogue() -> Catalogue<'static> {
    Catalogue {
        version: env!("CARGO_PKG_VERSION"),
        categories: CATEGORIES,
        catalog: CATALOG,
    }
}

fn sources(root: &Path) -> Result<Sources, String> {
    Ok(Sources {
        skill_md: read(root, paths::SKILL_MD)?,
        server_readme: read(root, paths::SERVER_README)?,
        server_agents: read(root, paths::SERVER_AGENTS)?,
        root_readme: read(root, paths::ROOT_README)?,
        docs_readme: read(root, paths::DOCS_README)?,
    })
}

fn write_all(root: &Path, generated: &[Output]) -> Result<usize, String> {
    let mut written = 0;
    for output in generated {
        let path = root.join(output.path);
        if std::fs::read_to_string(&path).is_ok_and(|current| current == output.content) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, &output.content).map_err(|error| format!("cannot write {}: {error}", output.path))?;
        println!("wrote {}", output.path);
        written += 1;
    }
    Ok(written)
}

fn check_all(root: &Path, generated: &[Output]) -> bool {
    let mut stale = false;
    for output in generated {
        let current = std::fs::read_to_string(root.join(output.path)).unwrap_or_default();
        let diff = unified_diff(output.path, &current, &output.content);
        if !diff.is_empty() {
            stale = true;
            print!("{diff}");
        }
    }
    if stale {
        eprintln!("The generated files are out of date. Fix with: {FIX}");
    }
    !stale
}

fn run_lint(root: &Path, generated: &[Output]) -> Result<bool, String> {
    let skill = read(root, paths::SKILL_MD)?;
    let install = read(root, paths::INSTALL_MD)?;
    let allow = read(root, paths::LINT_ALLOW)?;
    let registry = mcp_server::get_all_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    let catalogue = catalogue();
    let problems = lint(&LintInput {
        catalogue: &catalogue,
        registry_names: &registry,
        entry_names: &ENTRY_TOOL_NAMES,
        hand_written: &[(paths::SKILL_MD, skill.as_str()), (paths::INSTALL_MD, install.as_str())],
        outputs: generated,
        allow: &allow,
    });
    for problem in &problems {
        eprintln!("lint: {problem}");
    }
    Ok(problems.is_empty())
}

fn run(arguments: &Arguments) -> Result<bool, String> {
    let generated = outputs(&catalogue(), &sources(&arguments.root)?)?;
    match arguments.mode {
        Mode::Write => write_all(&arguments.root, &generated).map(|_| true),
        Mode::Check => Ok(check_all(&arguments.root, &generated)),
        Mode::Lint => run_lint(&arguments.root, &generated),
    }
}

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let parsed = match parse(&arguments) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&parsed) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
