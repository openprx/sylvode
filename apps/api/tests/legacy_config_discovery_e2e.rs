//! ADR-0020 D2: the API logs one `warn` line when it loads the legacy configuration file by
//! default discovery, and none when the file is named with `--config` or is the canonical one.
//!
//! The configuration points at a database port nothing listens on, so the API stops right
//! after startup logging with a connection error; the notice is emitted before that point.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NOTICE: &str = "legacy configuration file config/openpr.toml was discovered by default";

fn workdir(file_name: &str, tag: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("api-config-notice-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(dir.join("config"))?;
    std::fs::write(
        dir.join("config").join(file_name),
        "[database]\nurl = \"postgres://openpr:unused@127.0.0.1:1/openpr\"\nconnect_timeout_seconds = 1\nacquire_timeout_seconds = 1\n\n[auth]\njwt_secret = \"0123456789abcdef0123456789abcdef\"\n\n[logging]\nformat = \"text\"\n",
    )?;
    Ok(dir)
}

fn run(cwd: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    run_with_stderr(cwd, args, Stdio::piped())
}

fn run_with_stderr(cwd: &Path, args: &[&str], stderr: Stdio) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_api"))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()?;
    let started = Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() > Duration::from_mins(1) {
            child.kill()?;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(child.wait_with_output()?)
}

/// Whether the process got as far as connecting to the database, which is after the logger is
/// installed and after the notice would have been written.
fn reached_the_database(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stderr).contains("Connection Error")
}

fn notices(output: &Output) -> Vec<String> {
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    all.lines()
        .filter(|line| line.contains("legacy configuration file"))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn default_discovery_of_the_legacy_file_warns_once() -> TestResult {
    let dir = workdir("openpr.toml", "legacy")?;
    let output = run(&dir, &[])?;
    let found = notices(&output);
    assert_eq!(found.len(), 1, "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let notice = found.first().map(String::as_str).unwrap_or_default();
    assert!(notice.contains(NOTICE), "{notice}");
    assert!(notice.contains("config/sylvode.toml"), "{notice}");
    assert!(notice.contains("not removed before Sylvode v2.0"), "{notice}");
    assert!(notice.contains("WARN"), "{notice}");
    assert!(
        !output.status.success(),
        "the API reached a database that does not exist"
    );
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn an_explicit_legacy_path_and_the_canonical_file_do_not_warn() -> TestResult {
    let legacy = workdir("openpr.toml", "explicit")?;
    let output = run(&legacy, &["--config", "config/openpr.toml"])?;
    assert!(
        notices(&output).is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        reached_the_database(&output),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let canonical = workdir("sylvode.toml", "canonical")?;
    let output = run(&canonical, &[])?;
    assert!(
        notices(&output).is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        reached_the_database(&output),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(legacy)?;
    std::fs::remove_dir_all(canonical)?;
    Ok(())
}

/// The notice, and every other log line, is dropped when stderr cannot be written: with stderr
/// on `/dev/full` the process ends exactly as it does with a writable stderr (here: the database
/// connection error), never with a panic (exit 101) from the logger.
#[cfg(target_os = "linux")]
#[test]
fn an_unwritable_stderr_does_not_change_how_the_process_ends() -> TestResult {
    let dir = workdir("openpr.toml", "full")?;
    let writable = run(&dir, &[])?;
    assert_eq!(
        notices(&writable).len(),
        1,
        "{}",
        String::from_utf8_lossy(&writable.stderr)
    );
    let full = std::fs::OpenOptions::new().write(true).open("/dev/full")?;
    let unwritable = run_with_stderr(&dir, &[], Stdio::from(full))?;
    assert_ne!(unwritable.status.code(), Some(101), "the logger panicked");
    assert_eq!(unwritable.status.code(), writable.status.code());
    std::fs::remove_dir_all(dir)?;
    Ok(())
}
