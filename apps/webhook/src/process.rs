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
//!
//! # Never signalling a reused group id
//!
//! The group id is the leader's pid. The kernel does not reuse a pid while a process, a zombie
//! included, still has it as its pid or group id, so the id is safe to signal for as long as the
//! leader (our direct child) has not been reaped: alive or a zombie, it pins the id. Once the
//! leader is reaped and the last member of the group exits, the id is free and another,
//! unrelated, process may get it; a `killpg` sent then could reach that process. So the group is
//! only ever signalled while the leader is unreaped, and the leader is reaped last:
//!
//! - On Linux the leader's exit is noticed without reaping it (`waitid` with `WNOWAIT`), and
//!   whether the group still has a live member is read from `/proc` (the leader's zombie does not
//!   count). The leader is reaped only once the group is empty or has been sent `SIGKILL`.
//! - On other Unix systems the leader's exit can only be noticed by reaping it. A command that
//!   exits by itself is therefore reaped at once and its group is not signalled afterwards
//!   (anything it left running in the group keeps running there). On a timeout or cancellation
//!   the leader is still unreaped; the group gets `SIGTERM`, the whole grace period, then
//!   `SIGKILL`, and only then is the leader reaped.

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
    command.process_group(0);

    let mut child = command.spawn().map_err(|e| e.to_string())?;
    // Armed until just before the leader is reaped, so a run whose future is dropped (an aborted
    // task, a runtime shutting down) does not leave the group behind either. It is declared after
    // `child`, so it is dropped first, while the unreaped leader still pins the group id.
    let mut guard = GroupGuard::new(child.id());
    let stdout = tokio::spawn(read_tail(child.stdout.take()));
    let stderr = tokio::spawn(read_tail(child.stderr.take()));

    let (leader, ended) = tokio::select! {
        leader = wait_for_leader(&mut child) => (leader, Ended::Exited),
        () = tokio::time::sleep(timeout) => (Leader::Running, Ended::TimedOut),
        () = cancel => (Leader::Running, Ended::Cancelled),
    };

    // Also after a normal exit: anything the command left running in its group is stopped too.
    let reaped = terminate_tree(&mut child, &mut guard, leader, grace).await;
    let end = match (ended, reaped) {
        (Ended::TimedOut, _) => End::TimedOut,
        (Ended::Cancelled, _) => End::Cancelled,
        (Ended::Exited, Some(Ok(status))) => End::Exited(status),
        (Ended::Exited, Some(Err(e))) => End::WaitFailed(e.to_string()),
        (Ended::Exited, None) => End::WaitFailed("the command could not be reaped".to_string()),
    };

    Ok(Finished {
        end,
        stdout: join_reader(stdout).await,
        stderr: join_reader(stderr).await,
    })
}

/// Why the wait for the command ended.
#[derive(Clone, Copy)]
enum Ended {
    Exited,
    TimedOut,
    Cancelled,
}

/// What is known about the leader, the direct child.
enum Leader {
    /// Still running as far as is known.
    Running,
    /// Exited, not reaped: a zombie that still pins the group id. Only Linux can observe this
    /// state (`waitid` with `WNOWAIT`); elsewhere the leader is reaped as soon as it exits.
    #[cfg(target_os = "linux")]
    ExitedUnreaped,
    /// Reaped, with the result of reaping it. The group id may be reused once the group is empty.
    Reaped(std::io::Result<ExitStatus>),
}

/// Waits until the leader exits. On Linux it is left unreaped (see the module documentation).
#[cfg(target_os = "linux")]
async fn wait_for_leader(child: &mut tokio::process::Child) -> Leader {
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};

    let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
        return Leader::Reaped(child.wait().await);
    };
    let flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT;
    loop {
        match waitid(Id::Pid(nix::unistd::Pid::from_raw(pid)), flags) {
            Ok(WaitStatus::StillAlive) | Err(nix::errno::Errno::EINTR) => tokio::time::sleep(POLL_INTERVAL).await,
            Ok(_) => return Leader::ExitedUnreaped,
            Err(e) => {
                tracing::debug!("waiting for the command without reaping it failed: {e}");
                return Leader::Reaped(child.wait().await);
            }
        }
    }
}

/// Waits until the leader exits, reaping it: without `waitid` there is no other way to notice.
#[cfg(not(target_os = "linux"))]
async fn wait_for_leader(child: &mut tokio::process::Child) -> Leader {
    Leader::Reaped(child.wait().await)
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

/// Terminates the process group led by `child` and reaps `child`, signalling the group only while
/// the leader is unreaped. Returns the result of reaping the leader, or `None` when it was left
/// unreaped because it cannot be signalled (it is then reaped in the background once it exits).
async fn terminate_tree(
    child: &mut tokio::process::Child,
    guard: &mut GroupGuard,
    leader: Leader,
    grace: Duration,
) -> Option<std::io::Result<ExitStatus>> {
    use nix::sys::signal::Signal;

    let exited = match leader {
        Leader::Reaped(status) => {
            // The group id may already belong to someone else: never signal it now.
            guard.disarm();
            return Some(status);
        }
        #[cfg(target_os = "linux")]
        Leader::ExitedUnreaped => true,
        Leader::Running => false,
    };
    let Some(pgid) = guard.pgid else {
        // The pid could not be read; nothing can be signalled.
        return Some(reap(child, guard).await);
    };
    if exited && !group_has_live_member(pgid) {
        return Some(reap(child, guard).await);
    }
    match signal_group(pgid, Some(Signal::SIGTERM)) {
        Signalled::Delivered => {}
        Signalled::Gone => return Some(reap(child, guard).await),
        Signalled::Denied => return give_up(child, guard),
    }
    let deadline = tokio::time::Instant::now() + grace;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        if !group_has_live_member(pgid) {
            return Some(reap(child, guard).await);
        }
    }
    tracing::warn!(
        pgid,
        "process group still running {grace:?} after SIGTERM; sending SIGKILL"
    );
    if signal_group(pgid, Some(Signal::SIGKILL)) == Signalled::Denied {
        return give_up(child, guard);
    }
    Some(reap(child, guard).await)
}

/// The group cannot be signalled (`EPERM`, as for a set-user-ID program that changed its
/// credentials; macOS also answers `EPERM` for a group whose only member is the leader's zombie).
/// Nothing is signalled after this, so the leader may be reaped now if it has exited. Waiting for a
/// leader that is still running and might never exit would make the timeout meaningless, so such
/// a leader is left to `kill_on_drop` and to tokio, which reaps a dropped child once it exits.
fn give_up(child: &mut tokio::process::Child, guard: &mut GroupGuard) -> Option<std::io::Result<ExitStatus>> {
    guard.disarm();
    match child.try_wait() {
        Ok(Some(status)) => Some(Ok(status)),
        Ok(None) => {
            tracing::error!("the command's process group cannot be signalled; leaving it running");
            None
        }
        Err(e) => Some(Err(e)),
    }
}

/// Reaps the leader. The guard is disarmed first: once the leader is reaped the group id may be
/// reused, so nothing may signal it any more.
async fn reap(child: &mut tokio::process::Child, guard: &mut GroupGuard) -> std::io::Result<ExitStatus> {
    guard.disarm();
    let status = child.wait().await;
    if let Err(e) = &status {
        tracing::warn!("reaping the command failed: {e}");
    }
    status
}

/// The result of signalling a process group.
#[derive(Debug, PartialEq, Eq)]
enum Signalled {
    /// The signal was sent (or, with no signal, the group exists).
    Delivered,
    /// `ESRCH`: the group has no member left.
    Gone,
    /// `EPERM` or another error: the group exists, or may, but cannot be signalled.
    Denied,
}

/// Sends `signal` (or, with `None`, only checks for existence) to the process group `pgid`. Call
/// it only while the leader is unreaped (see the module documentation).
fn signal_group(pgid: i32, signal: Option<nix::sys::signal::Signal>) -> Signalled {
    classify(pgid, nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), signal))
}

fn classify(pgid: i32, result: nix::Result<()>) -> Signalled {
    match result {
        Ok(()) => Signalled::Delivered,
        Err(nix::errno::Errno::ESRCH) => Signalled::Gone,
        Err(nix::errno::Errno::EPERM) => {
            tracing::warn!(pgid, "not permitted to signal the process group (EPERM)");
            Signalled::Denied
        }
        Err(e) => {
            tracing::warn!(pgid, "signalling the process group failed: {e}");
            Signalled::Denied
        }
    }
}

/// Whether a process other than a zombie is in group `pgid`, read from `/proc`. When `/proc`
/// cannot be read the answer is yes, which only makes the caller wait the grace period and send
/// `SIGKILL`, both safe while the leader is unreaped.
#[cfg(target_os = "linux")]
fn group_has_live_member(pgid: i32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        name.to_str().is_some_and(|pid| pid.bytes().all(|b| b.is_ascii_digit()))
            && std::fs::read_to_string(entry.path().join("stat")).is_ok_and(|stat| live_in_group(&stat, pgid))
    })
}

/// Whether a `/proc/<pid>/stat` line describes a live (not zombie, not dead) member of `pgid`.
#[cfg(target_os = "linux")]
fn live_in_group(stat: &str, pgid: i32) -> bool {
    // After the command name, in parentheses (which may itself contain spaces or parentheses):
    // state, ppid, pgrp.
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    let mut fields = rest.split_whitespace();
    let state = fields.next();
    let group = fields.nth(1).and_then(|g| g.parse::<i32>().ok());
    group == Some(pgid) && !matches!(state, Some("Z" | "X" | "x"))
}

/// Without `/proc` there is no way to tell, so the group is assumed to have a live member.
#[cfg(all(unix, not(target_os = "linux")))]
const fn group_has_live_member(_pgid: i32) -> bool {
    true
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
        // Armed means the leader has not been reaped, so the group id is still pinned.
        if self.armed
            && let Some(pgid) = self.pgid
        {
            signal_group(pgid, Some(nix::sys::signal::Signal::SIGKILL));
        }
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
            Duration::from_mins(1),
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
            Duration::from_mins(1),
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
            Duration::from_mins(1),
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

    /// The group is signalled while the leader is still unreaped: a member that receives the
    /// `SIGTERM` after the leader exited by itself still finds the leader's zombie, which keeps
    /// the group id from being given to another process.
    #[tokio::test]
    async fn the_group_is_signalled_while_the_exited_leader_still_pins_its_id() {
        let dir = Dir::new();
        let file = dir.0.join("seen");
        let script = format!(
            "leader=$$; (trap 'if [ -e /proc/$leader/stat ]; then echo pinned; else echo released; fi > \"{f}\"; exit 0' TERM; \
             while :; do sleep 0.05; done) & echo started; exit 0",
            f = file.display()
        );
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", &script]);
        let finished = run(
            command,
            Duration::from_mins(1),
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
        assert_eq!(std::fs::read_to_string(&file).unwrap_or_default(), "pinned\n");
    }

    /// Noticing that the leader exited does not reap it: it stays a zombie until reaped.
    #[tokio::test]
    async fn the_leader_exit_is_noticed_without_reaping_it() {
        let mut command = tokio::process::Command::new("true");
        command.process_group(0);
        let mut child = command.spawn().expect("spawns");
        let pid = i32::try_from(child.id().expect("pid")).expect("pid fits");
        assert!(matches!(wait_for_leader(&mut child).await, Leader::ExitedUnreaped));
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("the zombie is still there");
        assert!(
            stat.rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
            "{stat}"
        );
        assert!(!group_has_live_member(pid), "a zombie leader is not a live member");
        assert!(child.wait().await.expect("reaped").success());
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn stat_lines_are_read_for_state_and_group() {
        assert!(live_in_group("42 (sh) S 1 42 42 0 -1", 42));
        assert!(live_in_group("43 (a) (b) c) R 42 42 42 0", 42));
        assert!(!live_in_group("42 (sh) Z 1 42 42 0 -1", 42));
        assert!(!live_in_group("42 (sh) X 1 42 42 0 -1", 42));
        assert!(!live_in_group("44 (sh) S 42 44 44 0 -1", 42));
        assert!(!live_in_group("garbage", 42));
    }

    /// `ESRCH` means the group is gone; `EPERM` means it exists but cannot be signalled, which
    /// must not be taken for gone (the caller would then wait for a leader it cannot stop).
    #[test]
    fn signalling_tells_a_missing_group_from_a_forbidden_one() {
        use nix::errno::Errno;
        assert_eq!(signal_group(i32::MAX - 7, None), Signalled::Gone);
        assert_eq!(classify(7, Ok(())), Signalled::Delivered);
        assert_eq!(classify(7, Err(Errno::ESRCH)), Signalled::Gone);
        assert_eq!(classify(7, Err(Errno::EPERM)), Signalled::Denied);
        assert_eq!(classify(7, Err(Errno::EINVAL)), Signalled::Denied);
    }

    /// When the group cannot be signalled, a leader that has exited meanwhile is still reaped
    /// with its status (macOS answers `EPERM` for a group left with only the leader's zombie), and
    /// a leader still running is left alone instead of being waited for.
    #[tokio::test]
    async fn giving_up_reaps_an_exited_leader_and_leaves_a_running_one() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exit 4"]).process_group(0);
        let mut child = command.spawn().expect("spawns");
        let mut guard = GroupGuard::new(child.id());
        assert!(matches!(wait_for_leader(&mut child).await, Leader::ExitedUnreaped));
        let reaped = give_up(&mut child, &mut guard).expect("reaped").expect("status");
        assert_eq!(reaped.code(), Some(4));
        assert!(!guard.armed);

        let mut command = tokio::process::Command::new("sleep");
        command.arg("30").process_group(0).kill_on_drop(true);
        let mut child = command.spawn().expect("spawns");
        let pid = i32::try_from(child.id().expect("pid")).expect("pid fits");
        let mut guard = GroupGuard::new(child.id());
        assert!(give_up(&mut child, &mut guard).is_none());
        assert!(!guard.armed, "nothing signals the group after giving up");
        drop(child);
        assert_eq!(wait_dead(&[pid], Duration::from_secs(2)).await, Vec::<i32>::new());
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
