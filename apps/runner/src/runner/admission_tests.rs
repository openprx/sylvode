//! Admission without waiting on a session's worker: the bounded answer, the queue cap, the full
//! channel, the hook that is dropped instead of held, and the answer the worker gets.

#![allow(clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use super::checks::BotCreds;
use super::files::StateDir;
use super::hooks::HookEvent;
use super::machine::State;
use super::session::{Cmd, EnqueueLimits, EnqueueRefusal, MemberStatus, SessionManager, TmuxTask, WorkerHandle};
use super::sv_client::SvClient;
use super::tmux::{NameSource, Slot, TmuxName};
use crate::background::Background;
use crate::config::Config;

const BOT: &str = "00000000-0000-4000-8000-000000000001";
const ANSWER: Duration = Duration::from_millis(100);
const SLACK: Duration = Duration::from_millis(50);

fn task(id: &str, slot: &Slot) -> TmuxTask {
    TmuxTask {
        task_id: id.to_string(),
        attempt: 1,
        max_attempts: 3,
        message: "go".to_string(),
        client: SvClient::new(reqwest::Client::new(), "http://127.0.0.1:1", "made-up", "mcp_stdio"),
        bot: BotCreds {
            bot_id: BOT.to_string(),
            api_url: "http://127.0.0.1:1".to_string(),
            workspace_id: "w".to_string(),
            token: "made-up".to_string(),
        },
        project_id: "p".to_string(),
        project_key: "SVWL".to_string(),
        slot: slot.clone(),
        tmux: TmuxName::derived(slot).expect("derived name"),
        name_source: NameSource::Derived,
        dir: PathBuf::from("/nonexistent"),
        dir_checked: true,
        cli: "claude".to_string(),
        program: PathBuf::from("/nonexistent/claude"),
        mcp_server: PathBuf::from("/nonexistent/mcp"),
    }
}

fn slot(key: &str) -> Slot {
    Slot::project(key, "claude").expect("slot")
}

/// A worker handle whose command channel nobody reads (an actor stalled in a long action).
fn stalled(capacity: usize) -> (WorkerHandle, mpsc::Receiver<Cmd>, watch::Sender<MemberStatus>) {
    let (tx, rx) = mpsc::channel(capacity);
    let (status_tx, status_rx) = watch::channel(MemberStatus::fresh());
    (WorkerHandle::new(tx, status_rx), rx, status_tx)
}

fn limits(max_queued: usize) -> EnqueueLimits {
    EnqueueLimits {
        answer: ANSWER,
        max_queued,
    }
}

fn queued_id(command: Cmd) -> String {
    match command {
        Cmd::Enqueue { task, .. } => task.task_id.clone(),
        _ => panic!("expected an enqueue"),
    }
}

/// A worker that answers every enqueue with the queue length so far.
fn answering(mut rx: mpsc::Receiver<Cmd>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut position = 0;
        while let Some(command) = rx.recv().await {
            if let Cmd::Enqueue { reply, .. } = command {
                position += 1;
                let _ = reply.send(position);
            }
        }
    })
}

fn manager(max_queued: u32) -> (SessionManager, tempfile_dir::Dir) {
    let dir = tempfile_dir::Dir::new();
    let config = Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n[security]\nallow_unsigned = true\n\
         [runner]\nenqueue_answer_ms = 100\nmax_queued_per_session = {max_queued}\n"
    ))
    .expect("config");
    let state = StateDir::open(dir.path()).expect("state dir");
    let manager = SessionManager::new(
        Arc::new(config),
        Arc::new(state),
        PathBuf::from("/nonexistent/runner"),
        None,
        Background::new(1),
        crate::handshake::ReportTrigger::default(),
    )
    .expect("manager");
    (manager, dir)
}

mod tempfile_dir {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sylvode-admission-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("create dir");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("chmod");
            Self(path.canonicalize().expect("canonical"))
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::test]
async fn an_answered_enqueue_carries_its_position() {
    let (handle, rx, _status) = stalled(8);
    let _worker = answering(rx);
    let s = slot("AAA");
    assert_eq!(handle.enqueue(task("t1", &s), limits(0)).await, Ok(Some(1)));
    assert_eq!(handle.enqueue(task("t2", &s), limits(0)).await, Ok(Some(2)));
}

#[tokio::test]
async fn a_stalled_worker_does_not_hold_the_answer_and_order_is_kept() {
    // Nobody reads the channel: the worker is inside an action that takes seconds.
    let (handle, mut rx, _status) = stalled(8);
    let s = slot("AAA");
    for id in ["t1", "t2", "t3"] {
        let queued = task(id, &s);
        let started = Instant::now();
        let answer = handle.enqueue(queued, limits(0)).await;
        let took = started.elapsed();
        assert_eq!(answer, Ok(None), "no position, but accepted");
        assert!(took >= ANSWER, "waits the bounded time: {took:?}");
        assert!(took <= ANSWER + SLACK, "answers within the bound: {took:?}");
    }
    // Once the worker reads again it finds the tasks in arrival order.
    let order: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).map(queued_id).collect();
    assert_eq!(order, ["t1", "t2", "t3"]);
}

#[tokio::test]
async fn zero_answer_time_waits_for_the_worker() {
    let (handle, mut rx, _status) = stalled(8);
    let s = slot("AAA");
    let waiting = tokio::spawn({
        let handle = handle.clone();
        async move {
            let unbounded = EnqueueLimits {
                answer: Duration::ZERO,
                max_queued: 0,
            };
            handle.enqueue(task("t1", &s), unbounded).await
        }
    });
    tokio::time::sleep(ANSWER * 3).await;
    assert!(!waiting.is_finished(), "still waiting after three answer times");
    let Some(Cmd::Enqueue { reply, .. }) = rx.recv().await else {
        panic!("the enqueue arrives");
    };
    let _ = reply.send(7);
    assert_eq!(waiting.await.expect("join"), Ok(Some(7)));
}

#[tokio::test]
async fn a_full_channel_refuses_an_enqueue_and_releases_what_it_counted() {
    let (handle, mut rx, _status) = stalled(2);
    let s = slot("AAA");
    assert_eq!(handle.enqueue(task("t1", &s), limits(0)).await, Ok(None));
    assert_eq!(handle.enqueue(task("t2", &s), limits(0)).await, Ok(None));
    assert_eq!(
        handle.enqueue(task("t3", &s), limits(0)).await,
        Err(EnqueueRefusal::Full)
    );
    // The refusal did not leave a count behind: with room again and a cap of 2, one more fits.
    drop(rx.try_recv().expect("t1"));
    drop(rx.try_recv().expect("t2"));
    assert_eq!(handle.enqueue(task("t4", &s), limits(2)).await, Ok(None));
}

#[tokio::test]
async fn a_session_at_its_cap_refuses_an_enqueue() {
    let (handle, _rx, status) = stalled(8);
    let s = slot("AAA");
    // The worker reports two queued tasks: a cap of 2 is reached.
    status.send_replace(MemberStatus::fresh().next(State::Busy, 2, Instant::now()));
    assert_eq!(
        handle.enqueue(task("t1", &s), limits(2)).await,
        Err(EnqueueRefusal::Capped)
    );
    // One below the cap takes it, and then the commands in flight count towards the cap.
    status.send_replace(MemberStatus::fresh().next(State::Busy, 1, Instant::now()));
    assert_eq!(handle.enqueue(task("t2", &s), limits(2)).await, Ok(None));
    assert_eq!(
        handle.enqueue(task("t3", &s), limits(2)).await,
        Err(EnqueueRefusal::Capped)
    );
    // No cap: always accepted while there is room.
    assert_eq!(handle.enqueue(task("t4", &s), limits(0)).await, Ok(None));
}

#[tokio::test]
async fn an_ended_worker_is_closed_not_back_pressure() {
    let (handle, rx, _status) = stalled(8);
    drop(rx);
    let s = slot("AAA");
    assert_eq!(
        handle.enqueue(task("t1", &s), limits(0)).await,
        Err(EnqueueRefusal::Closed)
    );
}

#[tokio::test]
async fn the_manager_applies_the_configured_cap_and_answer_time() {
    let (manager, _dir) = manager(1);
    let s = slot("AAA");
    let (handle, _rx, _status) = stalled(8);
    manager.install(&s, handle);
    let (first, second) = (task("t1", &s), task("t2", &s));
    let started = Instant::now();
    assert_eq!(manager.enqueue(first).await, Ok(None));
    assert!(started.elapsed() <= ANSWER + SLACK);
    // The first is still in the channel: it counts, so the second is over the cap of 1.
    assert_eq!(manager.enqueue(second).await, Err(EnqueueRefusal::Capped));
}

#[tokio::test]
async fn a_full_channel_drops_a_hook_and_other_sessions_still_get_theirs() {
    let (manager, _dir) = manager(0);
    let (a, b) = (slot("AAA"), slot("BBB"));
    let (handle_a, mut rx_a, _status_a) = stalled(1);
    let (handle_b, mut rx_b, _status_b) = stalled(8);
    manager.install(&a, handle_a);
    manager.install(&b, handle_b);
    for seq in 1..=3 {
        manager.deliver_hook(a.as_str(), HookEvent::Stop, seq, None);
        manager.deliver_hook(b.as_str(), HookEvent::Stop, seq, None);
    }
    // A's channel holds one hook; the other two were dropped, not waited for.
    assert_eq!(manager.hooks_dropped(), 2);
    assert!(matches!(rx_a.try_recv(), Ok(Cmd::Hook { seq: 1, .. })));
    assert!(rx_a.try_recv().is_err());
    // B got all three, in order.
    for want in 1..=3 {
        assert!(matches!(rx_b.try_recv(), Ok(Cmd::Hook { seq, .. }) if seq == want));
    }
    // A hook for a session nobody runs is ignored.
    manager.deliver_hook("NOPE-claude", HookEvent::Stop, 9, None);
    assert_eq!(manager.hooks_dropped(), 2);
}

#[test]
fn back_pressure_answers_503_without_a_body_and_a_late_worker_answers_null() {
    use super::enqueue_answer;
    use axum::http::StatusCode;
    let config = Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n[security]\nallow_unsigned = true\n\
         [[agents]]\nid = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"webhook\"\n\
         [agents.webhook]\nurl = \"http://127.0.0.1:1/x\"\n\
         [agents.route]\nbot_ids = [\"{BOT}\"]\n"
    ))
    .expect("config");
    let agent = &config.agents[0];
    for why in [EnqueueRefusal::Full, EnqueueRefusal::Capped] {
        assert_eq!(
            enqueue_answer(agent, "SVWL-claude", Err(why)),
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
    }
    let late = enqueue_answer(agent, "SVWL-claude", Ok(None)).expect("dispatched");
    assert_eq!(late["status"], "dispatched");
    assert!(late["queue_position"].is_null());
    let on_time = enqueue_answer(agent, "SVWL-claude", Ok(Some(2))).expect("dispatched");
    assert_eq!(on_time["queue_position"], 2);
    let closed = enqueue_answer(agent, "SVWL-claude", Err(EnqueueRefusal::Closed)).expect("refused");
    assert_eq!(closed["status"], "refused");
}
