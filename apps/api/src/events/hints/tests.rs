//! Unit tests of the hint stream that need no database: payload parsing, the delivery filter, the
//! limits, and the stream loop driven directly (a slow consumer, a lagging fan-out, shutdown).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::time::Duration;

use axum::body::Bytes;
use sea_orm::DatabaseConnection;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    End, Hint, HintHub, HintLimits, HintStream, HintTuning, LimitExceeded, ProjectFilter, Readable, Signal, frames_for,
    hello_frame, normalized, parse_bot, parse_hint, project_to_check,
};

fn id() -> Uuid {
    Uuid::new_v4()
}

fn text(frames: &[Bytes]) -> Vec<String> {
    frames
        .iter()
        .map(|frame| String::from_utf8(frame.to_vec()).expect("a frame is UTF-8"))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

#[test]
fn the_payloads_the_triggers_write_parse() {
    let (task, project, workspace, bot) = (id(), id(), id(), id());
    let payload = format!(r#"{{"k": "task", "id": "{task}", "p": "{project}", "w": "{workspace}", "b": "{bot}"}}"#);
    assert_eq!(
        parse_hint(&payload),
        Some(Hint::Task {
            id: task,
            project: Some(project),
            workspace,
            bot
        })
    );
    // A task without a project (the trigger writes null) still parses.
    let payload = format!(r#"{{"k":"task","id":"{task}","p":null,"w":"{workspace}","b":"{bot}"}}"#);
    assert!(matches!(parse_hint(&payload), Some(Hint::Task { project: None, .. })));

    let payload = format!(r#"{{"k":"issue","id":"{task}","p":"{project}","w":"{workspace}"}}"#);
    assert_eq!(
        parse_hint(&payload),
        Some(Hint::Issue {
            id: task,
            project,
            workspace
        })
    );
    assert_eq!(
        parse_hint(r#"{"k":"tools"}"#),
        Some(Hint::Tools {
            project: None,
            workspace: None
        })
    );
    let payload = format!(r#"{{"k":"tools","p":"{project}","w":"{workspace}"}}"#);
    assert_eq!(
        parse_hint(&payload),
        Some(Hint::Tools {
            project: Some(project),
            workspace: Some(workspace)
        })
    );
    assert_eq!(parse_bot(&format!(r#"{{"b":"{bot}"}}"#)), Some(bot));
}

#[test]
fn payloads_the_triggers_never_write_are_rejected() {
    let (a, b) = (id(), id());
    for payload in [
        "",
        "not json",
        "{}",
        r#"{"k":"nope"}"#,
        // A task needs its id, workspace and bot; an issue its id, project and workspace.
        &format!(r#"{{"k":"task","id":"{a}","w":"{b}"}}"#),
        &format!(r#"{{"k":"issue","id":"{a}","w":"{b}"}}"#),
        r#"{"k":"issue","id":"not-a-uuid","p":"x","w":"y"}"#,
    ] {
        assert_eq!(parse_hint(payload), None, "{payload}");
    }
    assert_eq!(parse_bot("{}"), None);
    assert_eq!(parse_bot(r#"{"b":"x"}"#), None);
}

// ---------------------------------------------------------------------------------------------
// The delivery filter
// ---------------------------------------------------------------------------------------------

#[test]
fn a_task_hint_goes_to_its_own_bot_only_as_task_and_inbox() {
    let (bot, other, workspace, project, task) = (id(), id(), id(), id(), id());
    let hint = |bot, workspace| Hint::Task {
        id: task,
        project: Some(project),
        workspace,
        bot,
    };
    let frames = text(&frames_for(hint(bot, workspace), bot, workspace, |_| false));
    assert_eq!(
        frames,
        vec![
            format!("event: task\ndata: {{\"id\":\"{task}\"}}\n\n"),
            "event: inbox\ndata: {}\n\n".to_string(),
        ],
        "a task hint is not tied to a readable project: the bot owns the task"
    );
    assert!(frames_for(hint(other, workspace), bot, workspace, |_| true).is_empty());
    assert!(frames_for(hint(bot, id()), bot, workspace, |_| true).is_empty());
}

#[test]
fn an_issue_hint_needs_the_workspace_and_a_readable_project() {
    let (bot, workspace, project, item) = (id(), id(), id(), id());
    let hint = |workspace, project| Hint::Issue {
        id: item,
        project,
        workspace,
    };
    let frames = text(&frames_for(hint(workspace, project), bot, workspace, |p| p == project));
    assert_eq!(frames, vec![format!("event: issue\ndata: {{\"id\":\"{item}\"}}\n\n")]);
    assert!(frames_for(hint(workspace, project), bot, workspace, |_| false).is_empty());
    assert!(frames_for(hint(workspace, project), bot, workspace, |p| p != project).is_empty());
    assert!(frames_for(hint(id(), project), bot, workspace, |_| true).is_empty());
}

#[test]
fn a_tools_hint_is_global_or_for_a_readable_project_of_the_workspace() {
    let (bot, workspace, project) = (id(), id(), id());
    let tools = |project, workspace| Hint::Tools { project, workspace };
    let sent = |hint| !frames_for(hint, bot, workspace, |p| p == project).is_empty();
    assert!(sent(tools(None, None)), "the global hint reaches every bot");
    assert!(sent(tools(Some(project), Some(workspace))));
    assert!(
        !sent(tools(Some(id()), Some(workspace))),
        "a project the bot cannot read"
    );
    assert!(
        !sent(tools(Some(project), Some(id()))),
        "a project of another workspace"
    );
    assert!(!sent(tools(None, Some(id()))));
    let frames = text(&frames_for(tools(None, None), bot, workspace, |_| false));
    assert_eq!(frames, vec!["event: tools\ndata: {}\n\n".to_string()]);
}

#[test]
fn hints_carry_ids_never_content() {
    let (bot, workspace, project, item) = (id(), id(), id(), id());
    let hints = [
        Hint::Task {
            id: item,
            project: Some(project),
            workspace,
            bot,
        },
        Hint::Issue {
            id: item,
            project,
            workspace,
        },
        Hint::Tools {
            project: None,
            workspace: None,
        },
    ];
    for hint in hints {
        for frame in text(&frames_for(hint, bot, workspace, |_| true)) {
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .expect("a data line");
            let value: serde_json::Value = serde_json::from_str(data).expect("JSON");
            let keys: Vec<&str> = value
                .as_object()
                .expect("an object")
                .keys()
                .map(String::as_str)
                .collect();
            assert!(keys.iter().all(|key| *key == "id"), "only an id may travel: {frame}");
        }
    }
}

#[test]
fn the_project_that_decides_delivery_is_named_only_for_issue_and_tools_hints() {
    let (workspace, project) = (id(), id());
    let issue = Hint::Issue {
        id: id(),
        project,
        workspace,
    };
    assert_eq!(project_to_check(issue, workspace), Some(project));
    assert_eq!(project_to_check(issue, id()), None, "another workspace needs no lookup");
    let tools = Hint::Tools {
        project: Some(project),
        workspace: Some(workspace),
    };
    assert_eq!(project_to_check(tools, workspace), Some(project));
    let global = Hint::Tools {
        project: None,
        workspace: None,
    };
    assert_eq!(project_to_check(global, workspace), None);
    let task = Hint::Task {
        id: id(),
        project: Some(project),
        workspace,
        bot: id(),
    };
    assert_eq!(project_to_check(task, workspace), None);
}

#[test]
fn the_hello_frame_names_the_bot_and_its_workspace() {
    let (bot, workspace) = (id(), id());
    assert_eq!(
        text(&[hello_frame(bot, workspace)]),
        vec![format!(
            "event: hello\ndata: {{\"bot_id\":\"{bot}\",\"workspace_id\":\"{workspace}\"}}\n\n"
        )]
    );
}

#[test]
fn readable_sets_compare_regardless_of_order() {
    let (a, b) = (id(), id());
    assert_eq!(
        normalized(ProjectFilter::Only(vec![a, b])),
        normalized(ProjectFilter::Only(vec![b, a, a]))
    );
    assert_ne!(normalized(ProjectFilter::Only(vec![a])), ProjectFilter::All);
}

// ---------------------------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------------------------

#[test]
fn a_bot_holds_at_most_its_own_streams_and_the_process_its_cap() {
    let hub = HintHub::new(
        HintLimits {
            per_bot: 2,
            per_process: 3,
        },
        HintTuning::default(),
    );
    let (a, b) = (id(), id());
    let first = hub.try_acquire(a).expect("first");
    let _second = hub.try_acquire(a).expect("second");
    assert_eq!(hub.try_acquire(a).err(), Some(LimitExceeded::Bot));
    let _third = hub.try_acquire(b).expect("another bot");
    assert_eq!(hub.open_streams(), 3);
    assert_eq!(hub.try_acquire(b).err(), Some(LimitExceeded::Process));
    drop(first);
    assert_eq!(hub.open_streams(), 2);
    assert!(hub.try_acquire(a).is_ok(), "a released place can be taken again");
}

#[test]
fn the_contract_limits_are_the_defaults() {
    let limits = HintLimits::from_runtime();
    assert_eq!(limits.per_bot, 64);
    assert_eq!(limits.per_process, 4_096);
    let tuning = HintTuning::default();
    assert_eq!(tuning.channel_capacity, 256);
    assert_eq!(tuning.lag_timeout, Duration::from_secs(30));
    assert_eq!(tuning.tick, Duration::from_secs(3));
    assert_eq!(tuning.readable_ttl, Duration::from_secs(2));
    assert_eq!(tuning.keepalive, Duration::from_secs(15));
    assert_eq!(super::BROADCAST_CAPACITY, 4_096);
}

// ---------------------------------------------------------------------------------------------
// The stream loop, without a database
// ---------------------------------------------------------------------------------------------

struct Loop {
    hub: std::sync::Arc<HintHub>,
    signals: broadcast::Sender<Signal>,
    rx: mpsc::Receiver<Bytes>,
    stream: Option<HintStream>,
    bot: Uuid,
    workspace: Uuid,
}

fn stream_loop(tuning: HintTuning, broadcast_capacity: usize) -> Loop {
    let hub = HintHub::new(HintLimits::from_runtime(), tuning);
    let (signals, receiver) = broadcast::channel(broadcast_capacity);
    let (tx, rx) = mpsc::channel(tuning.channel_capacity);
    let (bot, workspace) = (id(), id());
    let stream = HintStream {
        db: DatabaseConnection::Disconnected,
        bot_id: bot,
        workspace_id: workspace,
        token_hash: String::new(),
        tuning,
        signals: receiver,
        shutdown: hub.shutdown.subscribe(),
        tx,
        readable: Readable {
            filter: ProjectFilter::All,
            fetched: Instant::now(),
        },
        expiry: None,
        db_failing_since: None,
        sent: 0,
    };
    Loop {
        hub,
        signals,
        rx,
        stream: Some(stream),
        bot,
        workspace,
    }
}

fn quiet_tuning() -> HintTuning {
    HintTuning {
        // Timers far away: only the signals drive these tests.
        tick: Duration::from_hours(1),
        keepalive: Duration::from_hours(1),
        ..HintTuning::default()
    }
}

fn task_signal(l: &Loop) -> Signal {
    Signal::Hint(Hint::Task {
        id: id(),
        project: None,
        workspace: l.workspace,
        bot: l.bot,
    })
}

#[tokio::test]
async fn a_consumer_that_does_not_drain_is_ended_with_lagged_and_the_queue_stays_bounded() {
    let tuning = HintTuning {
        channel_capacity: 4,
        lag_timeout: Duration::from_millis(200),
        ..quiet_tuning()
    };
    let mut l = stream_loop(tuning, 4_096);
    // Far more frames than the queue holds, none of them read.
    for _ in 0..50 {
        l.signals.send(task_signal(&l)).expect("a receiver exists");
    }
    let stream = l.stream.take().expect("stream");
    let started = Instant::now();
    let (end, sent) = tokio::time::timeout(Duration::from_secs(5), stream.run())
        .await
        .expect("the stream ends by itself");
    assert_eq!(end, End::Lagged);
    assert_eq!(sent, 4, "exactly the queue capacity was accepted");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the lag timeout was waited"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(l.rx.len(), 4, "the queue never holds more than its capacity");
    assert_eq!(
        text(&[End::Lagged.final_frame().expect("a closing frame")]),
        vec!["event: lagged\ndata: {}\n\n".to_string()]
    );
}

#[tokio::test]
async fn a_consumer_that_drains_is_not_ended() {
    let tuning = HintTuning {
        channel_capacity: 4,
        lag_timeout: Duration::from_millis(200),
        ..quiet_tuning()
    };
    let mut l = stream_loop(tuning, 4_096);
    let stream = l.stream.take().expect("stream");
    let task = tokio::spawn(stream.run());
    for _ in 0..20 {
        l.signals.send(task_signal(&l)).expect("a receiver exists");
        let first = tokio::time::timeout(Duration::from_secs(2), l.rx.recv())
            .await
            .expect("a frame")
            .expect("open");
        assert!(text(&[first])[0].starts_with("event: task"));
        let second = tokio::time::timeout(Duration::from_secs(2), l.rx.recv())
            .await
            .expect("a frame")
            .expect("open");
        assert!(text(&[second])[0].starts_with("event: inbox"));
    }
    assert!(!task.is_finished());
    drop(l.rx);
    let (end, sent) = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("a client that left ends the stream")
        .expect("joined");
    assert_eq!(end, End::ClientGone);
    assert_eq!(sent, 40);
}

#[tokio::test]
async fn a_stream_that_falls_behind_the_shared_channel_ends_with_lagged() {
    let mut l = stream_loop(quiet_tuning(), 2);
    for _ in 0..10 {
        l.signals.send(task_signal(&l)).expect("a receiver exists");
    }
    let stream = l.stream.take().expect("stream");
    let (end, _) = tokio::time::timeout(Duration::from_secs(2), stream.run())
        .await
        .expect("ends");
    assert_eq!(end, End::Lagged, "a broadcast overflow means hints were lost");
}

#[tokio::test]
async fn a_lost_listener_or_an_unreadable_payload_ends_streams_with_lagged() {
    let mut l = stream_loop(quiet_tuning(), 16);
    // The same signal the hub sends when its listener reconnected.
    l.signals.send(Signal::Reset).expect("a receiver exists");
    let stream = l.stream.take().expect("stream");
    let (end, _) = tokio::time::timeout(Duration::from_secs(2), stream.run())
        .await
        .expect("ends");
    assert_eq!(end, End::Lagged);
}

#[tokio::test]
async fn an_unreadable_payload_on_the_hub_reaches_every_subscriber_as_a_reset() {
    let hub = HintHub::new(HintLimits::from_runtime(), quiet_tuning());
    let mut a = hub.signals.subscribe();
    let mut b = hub.signals.subscribe();
    hub.dispatch(super::HINT_CHANNEL, "{\"k\":\"issue\"}");
    hub.dispatch(super::ACCESS_CHANNEL, "garbage");
    hub.dispatch(super::TOKEN_CHANNEL, "{}");
    hub.dispatch("some_other_channel", "{}");
    for receiver in [&mut a, &mut b] {
        for _ in 0..3 {
            assert!(matches!(receiver.try_recv(), Ok(Signal::Reset)));
        }
        assert!(receiver.try_recv().is_err(), "an unknown channel is ignored");
    }
}

#[tokio::test]
async fn shutdown_ends_a_stream_with_the_shutdown_frame() {
    let mut l = stream_loop(quiet_tuning(), 16);
    let stream = l.stream.take().expect("stream");
    let task = tokio::spawn(stream.run());
    tokio::time::sleep(Duration::from_millis(50)).await;
    l.hub.shutdown();
    let (end, _) = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("ends")
        .expect("joined");
    assert_eq!(end, End::Shutdown);
    assert_eq!(
        text(&[end.final_frame().expect("frame")]),
        vec!["event: shutdown\ndata: {}\n\n".to_string()]
    );
}

#[tokio::test]
async fn a_hint_for_another_bot_or_workspace_is_dropped_and_leaves_the_stream_open() {
    let mut l = stream_loop(quiet_tuning(), 16);
    let stream = l.stream.take().expect("stream");
    let task = tokio::spawn(stream.run());
    l.signals
        .send(Signal::Hint(Hint::Task {
            id: id(),
            project: None,
            workspace: l.workspace,
            bot: id(),
        }))
        .expect("receiver");
    l.signals
        .send(Signal::Hint(Hint::Task {
            id: id(),
            project: None,
            workspace: id(),
            bot: l.bot,
        }))
        .expect("receiver");
    // Notifications about other bots change nothing here.
    l.signals.send(Signal::Access(id())).expect("receiver");
    l.signals.send(Signal::Token(id())).expect("receiver");
    // The one that counts, so the test knows the earlier ones were processed.
    l.signals.send(task_signal(&l)).expect("receiver");
    let first = tokio::time::timeout(Duration::from_secs(2), l.rx.recv())
        .await
        .expect("a frame")
        .expect("open");
    assert!(text(&[first])[0].starts_with("event: task"));
    let second = tokio::time::timeout(Duration::from_secs(2), l.rx.recv())
        .await
        .expect("a frame")
        .expect("open");
    assert!(text(&[second])[0].starts_with("event: inbox"));
    assert!(l.rx.try_recv().is_err(), "nothing else was queued");
    assert!(!task.is_finished());
    task.abort();
}

#[tokio::test]
async fn a_keep_alive_comment_is_sent_when_idle() {
    let tuning = HintTuning {
        keepalive: Duration::from_millis(100),
        ..quiet_tuning()
    };
    let mut l = stream_loop(tuning, 16);
    let stream = l.stream.take().expect("stream");
    let task = tokio::spawn(stream.run());
    let frame = tokio::time::timeout(Duration::from_secs(2), l.rx.recv())
        .await
        .expect("a keep-alive")
        .expect("open");
    assert_eq!(text(&[frame]), vec![": keep-alive\n\n".to_string()]);
    task.abort();
}

#[tokio::test]
async fn the_expiry_deadline_ends_the_stream_with_the_expired_reason() {
    let mut l = stream_loop(quiet_tuning(), 16);
    let mut stream = l.stream.take().expect("stream");
    stream.expiry = Some(Instant::now() + Duration::from_millis(100));
    let (end, _) = tokio::time::timeout(Duration::from_secs(2), stream.run())
        .await
        .expect("ends");
    assert_eq!(end, End::Revoked(Some("expired")));
    assert_eq!(
        text(&[end.final_frame().expect("frame")]),
        vec!["event: revoked\ndata: {\"reason\":\"expired\"}\n\n".to_string()]
    );
    assert_eq!(
        text(&[End::Revoked(None).final_frame().expect("frame")]),
        vec!["event: revoked\ndata: {}\n\n".to_string()]
    );
    assert!(End::ClientGone.final_frame().is_none());
}

#[tokio::test]
async fn a_database_that_keeps_failing_ends_the_stream_only_after_the_give_up_time() {
    let tuning = HintTuning {
        tick: Duration::from_millis(50),
        db_give_up: Duration::from_millis(400),
        ..quiet_tuning()
    };
    let mut l = stream_loop(tuning, 16);
    let stream = l.stream.take().expect("stream");
    let started = Instant::now();
    let (end, _) = tokio::time::timeout(Duration::from_secs(5), stream.run())
        .await
        .expect("ends");
    assert_eq!(end, End::Db, "a database failure is not a revocation");
    assert!(started.elapsed() >= Duration::from_millis(400));
}
