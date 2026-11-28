//! Unit tests of the hint stream that need no database: payload parsing, the delivery filter, the
//! limits, and the stream loop driven directly (a slow consumer, a lagging fan-out, shutdown).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use sea_orm::DatabaseConnection;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use uuid::Uuid;

use super::TokenState;
use super::{
    CachedReadable, CachedToken, End, Hint, HintHub, HintLimits, HintStream, HintTuning, LimitExceeded, ProjectFilter,
    Readable, Scope, Signal, frames_for, hello_frame, normalized, parse_bot, parse_hint, project_to_check,
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
    let payload = format!(r#"{{"k":"draft","id":"{task}","p":"{project}","w":"{workspace}"}}"#);
    assert_eq!(
        parse_hint(&payload),
        Some(Hint::Draft {
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
fn a_draft_hint_needs_the_workspace_and_a_readable_project() {
    let (bot, workspace, project, draft) = (id(), id(), id(), id());
    let hint = |workspace, project| Hint::Draft {
        id: draft,
        project,
        workspace,
    };
    let frames = text(&frames_for(hint(workspace, project), bot, workspace, |p| p == project));
    assert_eq!(frames, vec![format!("event: draft\ndata: {{\"id\":\"{draft}\"}}\n\n")]);
    assert!(frames_for(hint(workspace, project), bot, workspace, |_| false).is_empty());
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
    assert_eq!(tuning.tick, Duration::from_secs(2));
    assert!(
        tuning.tick + tuning.shared_ttl <= Duration::from_secs(3),
        "worst-case revocation latency with LISTEN off is the contract's 3 s"
    );
    assert_eq!(tuning.readable_ttl, Duration::from_secs(2));
    assert_eq!(tuning.shared_ttl, Duration::from_secs(1));
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
        hub: Arc::clone(&hub),
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
        announced: ProjectFilter::All,
        expiry: None,
        db_failing_since: None,
        full_since: None,
        lost: false,
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

#[tokio::test]
async fn shutdown_and_expiry_are_not_held_back_by_a_consumer_that_does_not_drain() {
    // A long lag timeout: only a loop that never waits on the client can end in time.
    let tuning = HintTuning {
        channel_capacity: 2,
        lag_timeout: Duration::from_mins(1),
        ..quiet_tuning()
    };
    let mut l = stream_loop(tuning, 4_096);
    for _ in 0..50 {
        l.signals.send(task_signal(&l)).expect("a receiver exists");
    }
    l.hub.shutdown();
    let stream = l.stream.take().expect("stream");
    let (end, _) = tokio::time::timeout(Duration::from_secs(2), stream.run())
        .await
        .expect("shutdown is not queued behind the client");
    assert_eq!(end, End::Shutdown);

    let mut l = stream_loop(tuning, 4_096);
    for _ in 0..50 {
        l.signals.send(task_signal(&l)).expect("a receiver exists");
    }
    let mut stream = l.stream.take().expect("stream");
    stream.expiry = Some(Instant::now());
    let (end, _) = tokio::time::timeout(Duration::from_secs(2), stream.run())
        .await
        .expect("expiry is not queued behind the client");
    assert_eq!(end, End::Revoked(Some("expired")));
}

#[tokio::test]
async fn a_frame_dropped_on_a_full_queue_ends_the_stream_at_the_lag_deadline_even_after_a_drain() {
    let tuning = HintTuning {
        channel_capacity: 2,
        lag_timeout: Duration::from_millis(300),
        ..quiet_tuning()
    };
    let mut l = stream_loop(tuning, 4_096);
    for _ in 0..3 {
        l.signals.send(task_signal(&l)).expect("a receiver exists");
    }
    let stream = l.stream.take().expect("stream");
    let task = tokio::spawn(stream.run());
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The client wakes up and drains everything that was queued; one frame was already lost.
    while l.rx.try_recv().is_ok() {}
    let (end, sent) = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("the stream ends")
        .expect("joined");
    assert_eq!(end, End::Lagged, "a client that missed a frame has to re-read");
    assert_eq!(sent, 2);
}

#[test]
fn a_cached_token_lookup_loses_the_time_it_has_been_cached_and_is_dropped_when_it_ran_out() {
    let cached = |state, age: Duration| CachedToken {
        hash: String::new(),
        state,
        fetched: Instant::now() - age,
        generation: 0,
    };
    let left = cached(TokenState::Valid(Some(Duration::from_secs(10))), Duration::from_secs(4))
        .aged()
        .expect("still valid");
    match left {
        TokenState::Valid(Some(rest)) => assert!(rest <= Duration::from_secs(6) && rest > Duration::from_secs(5)),
        other => panic!("unexpected state {other:?}"),
    }
    assert_eq!(
        cached(TokenState::Valid(Some(Duration::from_secs(1))), Duration::from_secs(2)).aged(),
        None,
        "a lookup whose token would have expired meanwhile is read again"
    );
    assert_eq!(
        cached(TokenState::Valid(None), Duration::from_mins(1)).aged(),
        Some(TokenState::Valid(None))
    );
    assert_eq!(
        cached(TokenState::Revoked, Duration::ZERO).aged(),
        Some(TokenState::Revoked)
    );
}

#[tokio::test]
async fn a_token_or_access_notification_makes_the_shared_lookups_of_that_bot_unusable() {
    let hub = HintHub::new(HintLimits::from_runtime(), quiet_tuning());
    let (a, b) = (id(), id());
    let (shared_a, shared_b) = (hub.shared_for(a), hub.shared_for(b));
    let generation = |s: &Arc<super::BotShared>| s.generation.load(std::sync::atomic::Ordering::Acquire);
    assert_eq!((generation(&shared_a), generation(&shared_b)), (0, 0));
    hub.signal(Signal::Token(a));
    assert_eq!((generation(&shared_a), generation(&shared_b)), (1, 0));
    hub.signal(Signal::Access(b));
    assert_eq!((generation(&shared_a), generation(&shared_b)), (1, 1));
    hub.signal(Signal::Hint(Hint::Tools {
        project: None,
        workspace: None,
    }));
    assert_eq!(
        (generation(&shared_a), generation(&shared_b)),
        (1, 1),
        "a plain hint keeps them"
    );
    hub.signal(Signal::Reset);
    assert_eq!((generation(&shared_a), generation(&shared_b)), (2, 2));
}

#[test]
fn a_place_is_released_with_its_permit_and_takes_the_bots_shared_lookups_with_it() {
    let hub = HintHub::new(
        HintLimits {
            per_bot: 2,
            per_process: 4,
        },
        quiet_tuning(),
    );
    let bot = id();
    let permit = hub.try_acquire(bot).expect("a place");
    let _ = hub.shared_for(bot);
    assert_eq!(hub.shared.lock().len(), 1);
    drop(permit);
    assert_eq!(hub.open_streams(), 0);
    assert!(hub.shared.lock().is_empty(), "no stream left, nothing shared is kept");
}

// ---------------------------------------------------------------------------------------------
// Open-body cap
// ---------------------------------------------------------------------------------------------

#[test]
fn an_open_body_keeps_its_place_after_the_stream_place_is_released_and_frees_it_on_drop() {
    let hub = HintHub::new(
        HintLimits {
            per_bot: 1,
            per_process: 2,
        },
        quiet_tuning(),
    );
    let (a, b) = (id(), id());
    // Per bot: BODY_CAP_FACTOR times the stream cap.
    let mut bodies = Vec::new();
    for _ in 0..super::BODY_CAP_FACTOR {
        let stream = hub.try_acquire(a).expect("a stream place");
        bodies.push(hub.try_acquire_body(a).expect("a body place"));
        // The stream ended: its place is gone, the body (socket) is still open.
        drop(stream);
    }
    assert_eq!(hub.open_streams(), 0);
    assert_eq!(hub.open_bodies(), 2);
    assert_eq!(hub.try_acquire_body(a).err(), Some(LimitExceeded::Bot));
    // Per process: BODY_CAP_FACTOR times the process cap (4); the bot above holds 2.
    let _b1 = hub.try_acquire_body(b).expect("another bot");
    let _b2 = hub.try_acquire_body(b).expect("another bot again");
    assert_eq!(hub.open_bodies(), 4);
    assert_eq!(hub.try_acquire_body(id()).err(), Some(LimitExceeded::Process));
    // Dropping a body (the connection let go of it) frees exactly one place.
    let released = bodies.pop();
    assert!(released.is_some());
    drop(released);
    assert_eq!(hub.open_bodies(), 3);
    assert!(hub.try_acquire_body(a).is_ok());
}

#[tokio::test]
async fn dropping_the_response_body_releases_its_place() {
    let hub = HintHub::new(HintLimits::from_runtime(), quiet_tuning());
    let bot = id();
    let permit = hub.try_acquire_body(bot).expect("a body place");
    let (_tx, rx) = mpsc::channel(1);
    let (_close_tx, close_rx) = tokio::sync::watch::channel(None);
    let body = super::body(rx, close_rx, permit);
    assert_eq!(hub.open_bodies(), 1, "the body holds its place while it exists");
    drop(body);
    assert_eq!(hub.open_bodies(), 0, "dropping the body releases it");
}

// ---------------------------------------------------------------------------------------------
// Per-workspace coalescing
// ---------------------------------------------------------------------------------------------

fn coalescing_tuning() -> HintTuning {
    HintTuning {
        // The window is one tick; short, because the tests wait it out in real time.
        tick: Duration::from_millis(150),
        ..quiet_tuning()
    }
}

fn issue_hint(workspace: Uuid, project: Uuid) -> String {
    format!(
        "{{\"k\":\"issue\",\"id\":\"{}\",\"p\":\"{project}\",\"w\":\"{workspace}\"}}",
        id()
    )
}

fn task_hint(workspace: Uuid, bot: Uuid) -> String {
    format!(
        "{{\"k\":\"task\",\"id\":\"{}\",\"w\":\"{workspace}\",\"b\":\"{bot}\"}}",
        id()
    )
}

fn drain(rx: &mut broadcast::Receiver<Signal>) -> Vec<Signal> {
    let mut out = Vec::new();
    while let Ok(signal) = rx.try_recv() {
        out.push(signal);
    }
    out
}

#[tokio::test]
async fn a_bulk_write_in_one_project_sends_a_burst_then_one_overflow_and_spares_other_projects() {
    let hub = HintHub::new(HintLimits::from_runtime(), coalescing_tuning());
    let mut rx = hub.signals.subscribe();
    let (workspace, noisy, quiet) = (id(), id(), id());
    let window = hub.tuning.tick;
    // Well under the workspace ceiling: the blast radius stays the one project.
    for _ in 0..(super::WORKSPACE_BURST - 56) {
        hub.dispatch(super::HINT_CHANNEL, &issue_hint(workspace, noisy));
    }
    // Another project's hint (same workspace) still goes through while the noisy one is held back.
    hub.dispatch(super::HINT_CHANNEL, &issue_hint(workspace, quiet));
    let first = drain(&mut rx);
    assert_eq!(first.len(), super::BURST + 1, "the burst plus the quiet project's hint");
    assert!(matches!(
        first.last(),
        Some(Signal::Hint(Hint::Issue { project, .. })) if *project == quiet
    ));
    // At the end of the window one overflow signal names the noisy project.
    tokio::time::sleep(window + Duration::from_millis(100)).await;
    let later = drain(&mut rx);
    assert_eq!(later.len(), 1);
    let scope = Scope::Project {
        workspace,
        project: noisy,
    };
    assert!(matches!(later[0], Signal::Overflow(s) if s == scope));
    // The next window starts fresh.
    hub.dispatch(super::HINT_CHANNEL, &issue_hint(workspace, noisy));
    assert!(matches!(drain(&mut rx)[..], [Signal::Hint(Hint::Issue { .. })]));
    assert!(
        hub.windows
            .lock()
            .0
            .get(&scope)
            .is_some_and(|w| w.passed == 1 && !w.overflowed)
    );
}

#[tokio::test]
async fn task_hints_are_counted_per_bot_and_tool_hints_are_not() {
    let hub = HintHub::new(HintLimits::from_runtime(), coalescing_tuning());
    let mut rx = hub.signals.subscribe();
    let (ws, noisy, quiet) = (id(), id(), id());
    for _ in 0..(super::BURST * 3) {
        hub.dispatch(super::HINT_CHANNEL, &task_hint(ws, noisy));
    }
    // Another bot's task and any number of tool hints are not held back by the noisy bot.
    hub.dispatch(super::HINT_CHANNEL, &task_hint(ws, quiet));
    for _ in 0..(super::BURST * 3) {
        hub.dispatch(super::HINT_CHANNEL, "{\"k\":\"tools\"}");
    }
    assert_eq!(drain(&mut rx).len(), super::BURST + 1 + super::BURST * 3);
    tokio::time::sleep(hub.tuning.tick + Duration::from_millis(100)).await;
    let later = drain(&mut rx);
    assert_eq!(later.len(), 1);
    assert!(matches!(
        later[0],
        Signal::Overflow(Scope::Bot { workspace, bot }) if workspace == ws && bot == noisy
    ));
}

#[tokio::test]
async fn a_quiet_project_never_overflows() {
    let hub = HintHub::new(HintLimits::from_runtime(), coalescing_tuning());
    let mut rx = hub.signals.subscribe();
    let (ws, project) = (id(), id());
    for _ in 0..super::BURST {
        hub.dispatch(super::HINT_CHANNEL, &issue_hint(ws, project));
    }
    tokio::time::sleep(hub.tuning.tick * 2 + Duration::from_millis(100)).await;
    let all = drain(&mut rx);
    assert_eq!(all.len(), super::BURST);
    assert!(all.iter().all(|s| matches!(s, Signal::Hint(_))));
}

#[tokio::test]
async fn an_overflow_ends_only_the_streams_that_could_have_received_the_held_back_hints() {
    let mut l = stream_loop(quiet_tuning(), 16);
    let mut stream = l.stream.take().expect("stream");
    let (a, b) = (id(), id());
    let project = |workspace, project| Signal::Overflow(Scope::Project { workspace, project });
    // Another workspace: never.
    assert!(stream.on_signal(Ok(project(id(), a))).await.is_ok());
    // Unrestricted reader: ends for any project of its workspace.
    assert!(matches!(
        stream.on_signal(Ok(project(l.workspace, a))).await,
        Err(End::Lagged)
    ));
    // A reader of project B only: ends for B, not for A.
    stream.readable.filter = ProjectFilter::Only(vec![b]);
    assert!(stream.on_signal(Ok(project(l.workspace, a))).await.is_ok());
    assert!(matches!(
        stream.on_signal(Ok(project(l.workspace, b))).await,
        Err(End::Lagged)
    ));
    // A task overflow concerns only the bot it names, in its workspace.
    let bot = |workspace, bot| Signal::Overflow(Scope::Bot { workspace, bot });
    assert!(stream.on_signal(Ok(bot(l.workspace, id()))).await.is_ok());
    assert!(stream.on_signal(Ok(bot(id(), l.bot))).await.is_ok());
    assert!(matches!(
        stream.on_signal(Ok(bot(l.workspace, l.bot))).await,
        Err(End::Lagged)
    ));
}

#[tokio::test]
async fn writes_spread_over_many_projects_are_bounded_per_workspace_and_spare_other_workspaces() {
    let hub = HintHub::new(HintLimits::from_runtime(), coalescing_tuning());
    let mut rx = hub.signals.subscribe();
    let (noisy, quiet) = (id(), id());
    // 200 projects of 20 hints each: every project is under its burst, the workspace is not.
    let projects: Vec<Uuid> = (0..200).map(|_| id()).collect();
    for _ in 0..20 {
        for project in &projects {
            hub.dispatch(super::HINT_CHANNEL, &issue_hint(noisy, *project));
        }
    }
    // Another workspace is not held back by the noisy one.
    hub.dispatch(super::HINT_CHANNEL, &issue_hint(quiet, id()));
    let first = drain(&mut rx);
    assert_eq!(first.len(), super::WORKSPACE_BURST + 1);
    assert!(matches!(
        first.last(),
        Some(Signal::Hint(Hint::Issue { workspace, .. })) if *workspace == quiet
    ));
    tokio::time::sleep(hub.tuning.tick + Duration::from_millis(100)).await;
    let later = drain(&mut rx);
    assert_eq!(later.len(), 1, "one workspace overflow, no project overflow");
    assert!(matches!(later[0], Signal::Overflow(Scope::Workspace(w)) if w == noisy));
}

#[tokio::test]
async fn a_workspace_overflow_ends_every_stream_of_that_workspace_only() {
    let mut l = stream_loop(quiet_tuning(), 16);
    let mut stream = l.stream.take().expect("stream");
    stream.readable.filter = ProjectFilter::Only(vec![id()]);
    assert!(
        stream
            .on_signal(Ok(Signal::Overflow(Scope::Workspace(id()))))
            .await
            .is_ok()
    );
    assert!(matches!(
        stream
            .on_signal(Ok(Signal::Overflow(Scope::Workspace(l.workspace))))
            .await,
        Err(End::Lagged)
    ));
}

#[test]
fn the_window_map_is_pruned_at_most_once_per_window() {
    let window = Duration::from_millis(100);
    let t0 = Instant::now();
    let mut windows = super::Windows::default();
    for _ in 0..1_100 {
        let ws = id();
        assert!(
            windows
                .admit(
                    ws,
                    Scope::Project {
                        workspace: ws,
                        project: id()
                    },
                    t0,
                    window
                )
                .is_none()
        );
    }
    assert_eq!(windows.1, Some(t0), "the first admit over the size pruned");
    let big = windows.0.len();
    assert!(big > 1_024, "nothing had expired yet");
    // Inside the same window: no second prune, even though the map is still large.
    let ws = id();
    windows.admit(
        ws,
        Scope::Project {
            workspace: ws,
            project: id(),
        },
        t0 + window / 2,
        window,
    );
    assert_eq!(windows.1, Some(t0));
    // A window later the expired entries are pruned, once.
    let later = t0 + window;
    windows.admit(
        ws,
        Scope::Project {
            workspace: ws,
            project: id(),
        },
        later,
        window,
    );
    assert_eq!(windows.1, Some(later));
    assert!(windows.0.len() < 10, "expired windows were removed");
}

#[tokio::test]
async fn an_overflow_decides_on_a_set_refreshed_when_stale_and_a_failed_lookup_ends_the_stream() {
    let mut l = stream_loop(quiet_tuning(), 16);
    let mut stream = l.stream.take().expect("stream");
    let (a, b) = (id(), id());
    let stale = Instant::now() - stream.tuning.readable_ttl * 4;
    stream.readable = Readable {
        filter: ProjectFilter::All,
        fetched: stale,
    };
    let overflow = Signal::Overflow(Scope::Project {
        workspace: l.workspace,
        project: a,
    });
    // No cached lookup and no database: the stale set is not used, the stream ends with Db.
    assert!(matches!(stream.on_signal(Ok(overflow)).await, Err(End::Db)));
    // A fresh shared lookup (the bot can now read only B): the stale All is replaced before the
    // decision, so the overflow of A does not end this stream. Without the refresh it would.
    *l.hub.shared_for(l.bot).readable.lock().await = Some(CachedReadable {
        readable: Readable {
            filter: ProjectFilter::Only(vec![b]),
            fetched: Instant::now(),
        },
        generation: 0,
    });
    assert!(stream.on_signal(Ok(overflow)).await.is_ok());
    assert_eq!(stream.readable.filter, ProjectFilter::Only(vec![b]));
}
