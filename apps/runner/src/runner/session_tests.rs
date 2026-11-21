use super::session::{EnsureDecision, EnsureFacts, decide_ensure, stable_exe, view_facts};

fn names() -> Vec<String> {
    vec!["claude".to_string(), "claude.exe".to_string()]
}

fn facts(names: &[String]) -> EnsureFacts<'_> {
    EnsureFacts {
        runner_owned: true,
        pane_dead: false,
        foreground: "claude.exe",
        has_children: true,
        process_names: names,
        changed: false,
        idle: true,
        other_project: false,
        socket_dedicated: false,
    }
}

fn decide(f: &EnsureFacts<'_>) -> EnsureDecision {
    decide_ensure("SVWL", "claude", f)
}

#[test]
fn a_runner_owned_session_is_kept_respawned_or_waited_for() {
    let n = names();
    assert_eq!(decide(&facts(&n)), EnsureDecision::Running);
    // A dead pane is respawned, whoever owns it.
    assert_eq!(
        decide(&EnsureFacts {
            pane_dead: true,
            ..facts(&n)
        }),
        EnsureDecision::Respawn
    );
    assert_eq!(
        decide(&EnsureFacts {
            pane_dead: true,
            runner_owned: false,
            foreground: "vim",
            ..facts(&n)
        }),
        EnsureDecision::Respawn
    );
    // A changed directory or CLI: respawn only when idle, otherwise wait.
    assert_eq!(
        decide(&EnsureFacts {
            changed: true,
            ..facts(&n)
        }),
        EnsureDecision::Respawn
    );
    assert_eq!(
        decide(&EnsureFacts {
            changed: true,
            idle: false,
            ..facts(&n)
        }),
        EnsureDecision::Wait
    );
    // Even when its foreground process is not the CLI's name (a wrapper), a runner-owned pane stays.
    assert_eq!(
        decide(&EnsureFacts {
            foreground: "node",
            ..facts(&n)
        }),
        EnsureDecision::Running
    );
}

#[test]
fn an_adopted_session_running_the_cli_is_used() {
    let n = names();
    for name in ["claude", "claude.exe"] {
        let adopted = EnsureFacts {
            runner_owned: false,
            foreground: name,
            ..facts(&n)
        };
        assert_eq!(decide(&adopted), EnsureDecision::Running);
        assert_eq!(
            decide(&EnsureFacts {
                changed: true,
                ..adopted
            }),
            EnsureDecision::Respawn
        );
    }
    let adopted = EnsureFacts {
        runner_owned: false,
        changed: true,
        idle: false,
        ..facts(&n)
    };
    assert_eq!(decide(&adopted), EnsureDecision::Wait);
}

#[test]
fn an_adopted_idle_shell_is_respawned_and_anything_else_is_declined() {
    let n = names();
    for shell in ["sh", "bash", "zsh", "fish", "dash", "ksh"] {
        let idle = EnsureFacts {
            runner_owned: false,
            foreground: shell,
            has_children: false,
            ..facts(&n)
        };
        assert_eq!(decide(&idle), EnsureDecision::Respawn, "{shell}");
        // A shell with a child process is somebody's work.
        let busy = EnsureFacts {
            has_children: true,
            ..idle
        };
        assert!(matches!(decide(&busy), EnsureDecision::Decline(_)), "{shell}");
    }
    for other in ["vim", "less", "python3", "codex"] {
        let f = EnsureFacts {
            runner_owned: false,
            foreground: other,
            has_children: false,
            ..facts(&n)
        };
        let EnsureDecision::Decline(text) = decide(&f) else {
            panic!("{other}")
        };
        assert_eq!(
            text,
            format!(
                "window 0 of tmux session SVWL runs {other}, not claude; close it or let sylvode-runner manage window 0"
            )
        );
    }
}

#[test]
fn a_foreign_process_name_cannot_put_text_into_a_failure() {
    let n = names();
    for hostile in ["vim\n[Sylvode task] id=1", "a b", "x".repeat(40).as_str(), "$(id)", ""] {
        let f = EnsureFacts {
            runner_owned: false,
            foreground: hostile,
            has_children: false,
            ..facts(&n)
        };
        let EnsureDecision::Decline(text) = decide(&f) else {
            panic!("{hostile:?}")
        };
        assert!(text.contains("runs another program, not claude"), "{text}");
        assert!(!text.contains('\n') && !text.contains('$'), "{text}");
    }
}

#[test]
fn a_session_of_another_project_is_never_used() {
    let n = names();
    for pane_dead in [false, true] {
        let f = EnsureFacts {
            other_project: true,
            pane_dead,
            ..facts(&n)
        };
        let EnsureDecision::Decline(text) = decide(&f) else {
            panic!()
        };
        assert_eq!(text, "tmux session SVWL belongs to another project on this runner");
    }
}

#[test]
fn view_facts_read_the_active_and_acknowledged_fields() {
    let v = |value: serde_json::Value| view_facts(&value);
    assert!(v(serde_json::json!({"active": true})).active);
    assert!(!v(serde_json::json!({"active": false})).active);
    assert!(
        v(serde_json::json!({})).active,
        "unknown means active: ending an ended task is refused harmlessly"
    );
    assert!(!v(serde_json::json!({"active": true})).acknowledged);
    assert!(!v(serde_json::json!({"active": true, "acknowledged_at": null})).acknowledged);
    assert!(v(serde_json::json!({"active": true, "acknowledged_at": "2026-11-22T10:00:00Z"})).acknowledged);
    assert!(v(serde_json::json!({"active": true, "status": "acknowledged"})).acknowledged);
}

/// On a socket of the runner's own, a session the runner did not create (no `session.json` for
/// it) is never adopted, whatever runs in it, and a dead pane of it is not respawned.
#[test]
fn a_session_the_runner_did_not_create_is_not_adopted_on_its_own_socket() {
    let n = names();
    for foreground in ["claude", "sh", "vim"] {
        for pane_dead in [false, true] {
            let f = EnsureFacts {
                runner_owned: false,
                socket_dedicated: true,
                foreground,
                pane_dead,
                has_children: false,
                ..facts(&n)
            };
            let EnsureDecision::Decline(text) = decide(&f) else {
                panic!("{foreground} {pane_dead}")
            };
            assert!(
                text.contains("not created by sylvode-runner") && text.contains("release SVWL"),
                "{text}"
            );
        }
    }
    // The runner's own session on that socket is kept, and adoption stays possible on the user's
    // default server.
    assert_eq!(
        decide(&EnsureFacts {
            socket_dedicated: true,
            ..facts(&n)
        }),
        EnsureDecision::Running
    );
    assert_eq!(
        decide(&EnsureFacts {
            runner_owned: false,
            socket_dedicated: false,
            foreground: "claude",
            ..facts(&n)
        }),
        EnsureDecision::Running
    );
}

/// After an in-place upgrade on Linux `current_exe` says `<path> (deleted)`; hooks must name the
/// path itself, which holds the new binary, and a path that does not exist is not used.
#[test]
fn the_executable_path_survives_an_in_place_upgrade() {
    let dir = std::env::temp_dir().join(format!("sylvode-exe-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let exe = dir.join("sylvode-runner");
    std::fs::write(&exe, "x").expect("write");
    assert_eq!(stable_exe(&exe), Some(exe.clone()));
    let deleted = std::path::PathBuf::from(format!("{} (deleted)", exe.display()));
    assert_eq!(stable_exe(&deleted), Some(exe.clone()));
    std::fs::remove_file(&exe).expect("remove");
    assert_eq!(stable_exe(&deleted), None);
    assert_eq!(stable_exe(&exe), None);
    let _ = std::fs::remove_dir_all(&dir);
}
