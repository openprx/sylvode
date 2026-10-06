//! Runs an external command so that its whole process tree can be stopped.
//!
//! On Unix every command is started as the leader of a new process group. When the run times out,
//! is cancelled (graceful shutdown), or ends on its own, the whole group is sent `SIGTERM`, then
//! `SIGKILL` after a grace period if any member is still alive. Killing only the direct child is
//! not enough: an executor such as an npm-installed `codex` is a launcher that starts the real
//! agent as a child, and an agent's tool calls are processes of their own. Those would otherwise
//! keep running, with the agent's permissions, after the run was reported as timed out.
//!
//! A process that deliberately leaves the group (`setsid`, `setpgid`) is out of reach of this
//! mechanism. On platforms other than Unix only the direct child is killed.

use std::future::Future;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

/// How long a process group gets between `SIGTERM` and `SIGKILL`.
pub const TERMINATION_GRACE: Duration = Duration::from_secs(5);

/// The most output kept per stream; older bytes are dropped, as only the tail is reported.
const MAX_CAPTURED_BYTES: usize = 1024 * 1024;

/// How long the output readers may take to finish once the process group is gone. A process that
/// escaped the group can hold a pipe open forever; its output is then cut off.
const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How a run ended.
#[derive(Debug)]
pub enum End {
    /// The command exited by itself.
    Exited(ExitStatus),
    /// The timeout elapsed and the process group was terminated.
    TimedOut,
    /// The run was cancelled (graceful shutdown) and the process group was terminated.
    Cancelled,
    /// Waiting for the command failed; the process group was terminated.
    WaitFailed(String),
}

/// The end of a run and the tail of what it wrote.
#[derive(Debug)]
pub struct Finished {
    pub end: End,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs `command` with stdin closed and stdout and stderr captured, until it exits, `timeout`
/// elapses or `cancel` resolves. In every case the command's process group is terminated before
/// this returns (`SIGTERM`, then `SIGKILL` after `grace`), so nothing it started keeps running.
pub async fn run(
    mut command: tokio::process::Command,
    timeout: Duration,
    grace: Duration,
    cancel: impl Future<Output = ()>,
) -> Result<Finished, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(|e| e.to_string())?;
    // Armed until the group has been terminated, so a run whose future is dropped (an aborted
    // task, a runtime shutting down) does not leave the group behind either.
    let mut guard = GroupGuard::new(child.id());
    let stdout = tokio::spawn(read_tail(child.stdout.take()));
    let stderr = tokio::spawn(read_tail(child.stderr.take()));

    let end = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => End::Exited(status),
            Err(e) => End::WaitFailed(e.to_string()),
        },
        () = tokio::time::sleep(timeout) => End::TimedOut,
        () = cancel => End::Cancelled,
    };

    // Also after a normal exit: anything the command left running in its group is stopped too.
    terminate_tree(&mut child, guard.pgid, grace).await;
    guard.disarm();

    Ok(Finished {
        end,
        stdout: join_reader(stdout).await,
        stderr: join_reader(stderr).await,
    })
}

async fn read_tail(stream: Option<impl AsyncRead + Unpin>) -> Vec<u8> {
    let Some(mut stream) = stream else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                kept.extend_from_slice(chunk.get(..n).unwrap_or_default());
                if kept.len() > MAX_CAPTURED_BYTES {
                    let excess = kept.len() - MAX_CAPTURED_BYTES;
                    kept.drain(..excess);
                }
            }
        }
    }
    kept
}

async fn join_reader(mut reader: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    match tokio::time::timeout(READER_DRAIN_TIMEOUT, &mut reader).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            tracing::warn!("output reader failed: {e}");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!("output still open after the process group ended; output cut off");
            reader.abort();
            Vec::new()
        }
    }
}

/// Terminates the process group led by `child` and reaps `child`.
#[cfg(unix)]
async fn terminate_tree(child: &mut tokio::process::Child, pgid: Option<i32>, grace: Duration) {
    use nix::sys::signal::Signal;

    let Some(pgid) = pgid else {
        // The child was already reaped before its pid could be read; nothing is left to signal.
        return;
    };
    if !signal_group(pgid, Some(Signal::SIGTERM)) {
        // No member is left; the leader may still need reaping.
        reap(child).await;
        return;
    }
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        // Reap the leader as soon as it exits, so it does not count as a live member.
        if let Err(e) = child.try_wait() {
            tracing::debug!("polling the command failed: {e}");
        }
        if !signal_group(pgid, None) {
            reap(child).await;
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    tracing::warn!(
        pgid,
        "process group still running {grace:?} after SIGTERM; sending SIGKILL"
    );
    signal_group(pgid, Some(Signal::SIGKILL));
    reap(child).await;
}

/// Without process groups only the direct child can be killed.
#[cfg(not(unix))]
async fn terminate_tree(child: &mut tokio::process::Child, _pgid: Option<i32>, _grace: Duration) {
    if let Err(e) = child.start_kill() {
        tracing::debug!("kill child: {e}");
    }
    reap(child).await;
}

async fn reap(child: &mut tokio::process::Child) {
    if let Err(e) = child.wait().await {
        tracing::warn!("reaping the command failed: {e}");
    }
}

/// Sends `signal` (or, with `None`, only checks for existence) to the process group `pgid`.
/// Returns whether the group still has a member.
#[cfg(unix)]
fn signal_group(pgid: i32, signal: Option<nix::sys::signal::Signal>) -> bool {
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), signal) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(e) => {
            tracing::warn!(pgid, "signalling the process group failed: {e}");
            false
        }
    }
}

/// Kills the process group with `SIGKILL` when dropped while still armed.
struct GroupGuard {
    pgid: Option<i32>,
    armed: bool,
}

impl GroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self {
            pgid: pid.and_then(|pid| i32::try_from(pid).ok()),
            armed: true,
        }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.armed
            && let Some(pgid) = self.pgid
        {
            signal_group(pgid, Some(nix::sys::signal::Signal::SIGKILL));
        }
        #[cfg(not(unix))]
        let _ = (self.armed, self.pgid);
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("sylvode-webhook-process-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("create dir");
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn alive(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|state| state != "Z" && state != "X")
        })
    }

    fn pids(file: &Path) -> Vec<i32> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    async fn wait_dead(pids: &[i32], within: Duration) -> Vec<i32> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let alive: Vec<i32> = pids.iter().copied().filter(|p| alive(*p)).collect();
            if alive.is_empty() || tokio::time::Instant::now() >= deadline {
                return alive;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A shell that starts two grandchildren (one of them in a subshell), records their pids and
    /// then waits; `extra` runs first.
    fn tree(dir: &Dir, extra: &str) -> (tokio::process::Command, PathBuf) {
        let file = dir.0.join("pids");
        let script = format!(
            "{extra}sleep 30 & echo $! >> '{f}'; (sleep 30; true) & echo $! >> '{f}'; echo started; wait",
            f = file.display()
        );
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", &script]);
        (command, file)
    }

    async fn started(file: &Path) -> Vec<i32> {
        for _ in 0..200 {
            let found = pids(file);
            if found.len() >= 2 {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the grandchildren did not start");
    }

    #[tokio::test]
    async fn a_timeout_terminates_every_grandchild() {
        let dir = Dir::new();
        let (command, file) = tree(&dir, "");
        let finished = run(
            command,
            Duration::from_millis(500),
            TERMINATION_GRACE,
            std::future::pending(),
        )
        .await
        .expect("spawns");
        assert!(matches!(finished.end, End::TimedOut), "{:?}", finished.end);
        assert_eq!(
            String::from_utf8_lossy(&finished.stdout),
            "started\n",
            "output before the timeout is kept"
        );
        let grandchildren = started(&file).await;
        assert_eq!(
            wait_dead(&grandchildren, Duration::from_secs(2)).await,
            Vec::<i32>::new()
        );
    }

    /// A grandchild that ignores SIGTERM is killed with SIGKILL once the grace period is over.
    #[tokio::test]
    async fn a_grandchild_ignoring_sigterm_is_killed_after_the_grace_period() {
        let dir = Dir::new();
        let (command, file) = tree(&dir, "trap '' TERM; ");
        let begun = tokio::time::Instant::now();
        let finished = run(
            command,
            Duration::from_millis(300),
            Duration::from_millis(300),
            std::future::pending(),
        )
        .await
        .expect("spawns");
        assert!(matches!(finished.end, End::TimedOut), "{:?}", finished.end);
        assert!(begun.elapsed() < Duration::from_secs(3), "{:?}", begun.elapsed());
        let grandchildren = started(&file).await;
        assert_eq!(
            wait_dead(&grandchildren, Duration::from_secs(2)).await,
            Vec::<i32>::new()
        );
    }

    #[tokio::test]
    async fn cancellation_terminates_every_grandchild() {
        let dir = Dir::new();
        let (command, file) = tree(&dir, "");
        let finished = run(
            command,
            Duration::from_secs(60),
            TERMINATION_GRACE,
            tokio::time::sleep(Duration::from_millis(300)),
        )
        .await
        .expect("spawns");
        assert!(matches!(finished.end, End::Cancelled), "{:?}", finished.end);
        let grandchildren = started(&file).await;
        assert_eq!(
            wait_dead(&grandchildren, Duration::from_secs(2)).await,
            Vec::<i32>::new()
        );
    }

    /// A command that exits but leaves a background process holding its stdout open neither
    /// hangs the run nor leaves the process running.
    #[tokio::test]
    async fn a_process_left_behind_after_a_normal_exit_is_terminated() {
        let dir = Dir::new();
        let file = dir.0.join("pids");
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", &format!("sleep 30 & echo $! > '{}'; echo done", file.display())]);
        let begun = tokio::time::Instant::now();
        let finished = run(
            command,
            Duration::from_secs(60),
            TERMINATION_GRACE,
            std::future::pending(),
        )
        .await
        .expect("spawns");
        assert!(
            matches!(finished.end, End::Exited(status) if status.success()),
            "{:?}",
            finished.end
        );
        assert!(begun.elapsed() < Duration::from_secs(3), "{:?}", begun.elapsed());
        assert_eq!(String::from_utf8_lossy(&finished.stdout), "done\n");
        let left = pids(&file);
        assert_eq!(left.len(), 1);
        assert_eq!(wait_dead(&left, Duration::from_secs(2)).await, Vec::<i32>::new());
    }

    /// A run whose future is dropped half-way (its task aborted) still takes the whole process
    /// group with it, not only the direct child that `kill_on_drop` covers.
    #[tokio::test]
    async fn dropping_a_run_kills_every_grandchild() {
        let dir = Dir::new();
        let (command, file) = tree(&dir, "");
        let task = tokio::spawn(run(
            command,
            Duration::from_secs(60),
            TERMINATION_GRACE,
            std::future::pending(),
        ));
        let grandchildren = started(&file).await;
        task.abort();
        assert!(task.await.is_err_and(|e| e.is_cancelled()));
        assert_eq!(
            wait_dead(&grandchildren, Duration::from_secs(2)).await,
            Vec::<i32>::new()
        );
    }

    #[tokio::test]
    async fn stdin_is_closed_and_exit_status_is_reported() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "cat; echo err >&2; exit 3"]);
        let finished = run(
            command,
            Duration::from_secs(10),
            TERMINATION_GRACE,
            std::future::pending(),
        )
        .await
        .expect("spawns");
        assert!(
            matches!(finished.end, End::Exited(status) if status.code() == Some(3)),
            "{:?}",
            finished.end
        );
        assert!(finished.stdout.is_empty(), "stdin is /dev/null, so cat reads nothing");
        assert_eq!(finished.stderr, b"err\n");
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error() {
        let command = tokio::process::Command::new("/nonexistent/sylvode-webhook-test-program");
        assert!(
            run(
                command,
                Duration::from_secs(1),
                TERMINATION_GRACE,
                std::future::pending()
            )
            .await
            .is_err()
        );
    }
}
