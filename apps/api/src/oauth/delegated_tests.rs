//! Unit and database tests of the delegated-access building blocks. The behaviour of the whole
//! branch through the real router is tested in `delegated_router_tests` next to `main.rs`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::Method;
use platform::app::AppState;
use platform::auth::JwtClaims;
use platform::config::{AppConfig, Secret};
use uuid::Uuid;

use super::delegated::{
    DELEGATED_ROUTES, DelegatedAuthContext, Grantee, ProjectRef, Required, Resolver, Target, USE_RECORD_INTERVAL,
    UseThrottle, lookup, parse_identifier, refuse_agent_dispatch, resolve_target, with_context,
};
use super::scopes::{ProjectScope, Scope, ScopeSet, TokenResource};
use super::test_support::{Start, oauth_scratch_or_skip};
use crate::middleware::bot_auth::EventSurface;
use crate::middleware::bot_auth::require_workspace_access_from_auth;
use crate::routes::context::tenant_fixture::{exec, seed_tenant, seed_work_item};

fn context(user_id: Uuid, workspace_id: Uuid, projects: ProjectScope) -> DelegatedAuthContext {
    DelegatedAuthContext {
        grant_id: Uuid::new_v4(),
        token_id: Uuid::new_v4(),
        client_id: Arc::from("ramflux"),
        user_id,
        workspace_id,
        scopes: ScopeSet::all(),
        projects,
        resource: TokenResource::Api,
        surface: EventSurface::Rest,
        tool_name: None,
        request_id: Uuid::new_v4(),
    }
}

fn params(pairs: &[(&str, String)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}

#[test]
fn the_table_has_no_duplicates_and_every_pattern_names_its_resolver_parameter() {
    let mut seen = BTreeSet::new();
    for entry in DELEGATED_ROUTES {
        assert!(
            seen.insert((entry.method, entry.pattern)),
            "{} {} twice",
            entry.method,
            entry.pattern
        );
        assert!(entry.pattern.starts_with("/api/v1/"), "{}", entry.pattern);
        let parameter = match entry.resolver {
            Resolver::Workspace(n)
            | Resolver::Project(n)
            | Resolver::Issue(n)
            | Resolver::IssueIdentifier(n)
            | Resolver::Notification(n) => Some(n),
            Resolver::SelfOnly | Resolver::GrantSelf => None,
        };
        if let Some(name) = parameter {
            assert!(
                entry.pattern.contains(&format!("{{{name}}}")),
                "{} does not contain {{{name}}}",
                entry.pattern
            );
        }
    }
}

#[test]
fn the_table_reaches_no_administration_secret_or_deletion_and_writes_need_write_scopes() {
    for entry in DELEGATED_ROUTES {
        for forbidden in [
            "/admin",
            "/bots",
            "/webhooks",
            "/secret",
            "/connected-apps",
            "/members",
            "/import",
            "/export",
            "/plugins",
            "/ai/tasks/",
            "/search",
        ] {
            assert!(
                !entry.pattern.contains(forbidden),
                "{} names {forbidden}",
                entry.pattern
            );
        }
        if entry.pattern.ends_with("/ai/tasks") {
            assert_eq!(entry.method, "GET", "tasks are listed, never created");
        }
        if entry.method == "DELETE" {
            assert_eq!(
                entry.pattern, "/api/v1/issues/{issue_id}/labels/{label_id}",
                "only a label is removed"
            );
        }
        if entry.method != "GET" {
            let Required::One(scope) = entry.required else {
                panic!("{} {} needs a specific scope", entry.method, entry.pattern);
            };
            assert!(
                scope.as_str().ends_with(":write"),
                "{} {}: {}",
                entry.method,
                entry.pattern,
                scope.as_str()
            );
        }
        assert!(
            !matches!(
                entry.required,
                Required::One(Scope::OfflineAccess | Scope::SurfacesEmbed)
            ),
            "{}",
            entry.pattern
        );
    }
}

#[test]
fn a_lookup_matches_the_template_exactly_and_the_method_exactly() {
    assert!(lookup(&Method::GET, "/api/v1/issues/{id}").is_some());
    assert!(lookup(&Method::PUT, "/api/v1/issues/{id}").is_some());
    // The raw URL is never a key, the method is part of it, and HEAD is not GET.
    assert!(lookup(&Method::GET, "/api/v1/issues/7a0b3c1e-0000-0000-0000-000000000000").is_none());
    assert!(lookup(&Method::DELETE, "/api/v1/issues/{id}").is_none());
    assert!(lookup(&Method::HEAD, "/api/v1/issues/{id}").is_none());
    assert!(lookup(&Method::GET, "/API/V1/issues/{id}").is_none());
    assert!(lookup(&Method::GET, "/api/v1/issues/{id}/").is_none());
    assert!(lookup(&Method::GET, "/api/v1/issues/{issue_id}").is_none());
    assert!(lookup(&Method::GET, "").is_none());
}

#[test]
fn required_scopes_follow_the_narrow_hierarchy() {
    let write = ScopeSet::from_scopes(&[Scope::WorkItemsWrite]);
    assert!(Required::One(Scope::WorkItemsRead).satisfied_by(write));
    assert!(Required::One(Scope::WorkItemsWrite).satisfied_by(write));
    let read = ScopeSet::from_scopes(&[Scope::WorkItemsRead]);
    assert!(!Required::One(Scope::WorkItemsWrite).satisfied_by(read));
    let notifications = ScopeSet::from_scopes(&[Scope::NotificationsWrite]);
    assert!(!Required::One(Scope::NotificationsRead).satisfied_by(notifications));
    let any = Required::AnyOf(&[Scope::NotificationsRead, Scope::TasksRead]);
    assert!(any.satisfied_by(ScopeSet::from_scopes(&[Scope::TasksRead])));
    assert!(!any.satisfied_by(ScopeSet::from_scopes(&[Scope::ProjectsRead])));
    assert!(Required::AnyScope.satisfied_by(ScopeSet::from_scopes(&[Scope::ProjectsRead])));
    assert!(!Required::AnyScope.satisfied_by(ScopeSet::EMPTY));
    assert_eq!(
        Required::One(Scope::WorkItemsWrite)
            .challenge()
            .map(ScopeSet::to_scope_string)
            .as_deref(),
        Some("work_items:read work_items:write")
    );
    assert_eq!(Required::AnyScope.challenge(), None);
}

#[test]
fn identifiers_split_like_the_handler_does() {
    assert_eq!(parse_identifier("PRX-42"), Some(("PRX", 42)));
    assert_eq!(parse_identifier("a-b-7"), Some(("a-b", 7)));
    for bad in ["PRX", "PRX-", "-4", "PRX-x", "PRX-4.5", ""] {
        assert_eq!(parse_identifier(bad), None, "{bad}");
    }
}

#[test]
fn a_target_is_reachable_inside_the_workspace_and_the_project_set_only() {
    let (ws, other_ws, p1, p2) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let selected = ProjectScope::Selected(Arc::from(vec![p1]));
    let reach = |target: Target, projects: &ProjectScope| target.is_reachable_by(ws, projects);
    let project = |workspace_id, project| Target {
        workspace_id,
        project: ProjectRef::Project(project),
    };
    assert!(reach(project(ws, p1), &selected));
    assert!(!reach(project(ws, p2), &selected));
    assert!(!reach(project(other_ws, p1), &selected));
    assert!(reach(project(ws, p2), &ProjectScope::All));
    assert!(!reach(project(other_ws, p2), &ProjectScope::All));
    let bare = |workspace_id| Target {
        workspace_id,
        project: ProjectRef::NotProjectBound,
    };
    assert!(reach(bare(ws), &selected) && !reach(bare(other_ws), &selected));
    let unresolved = Target {
        workspace_id: ws,
        project: ProjectRef::Unresolvable,
    };
    assert!(
        !reach(unresolved, &selected),
        "an object with no project is not in a selected grant"
    );
    assert!(reach(unresolved, &ProjectScope::All));
}

#[test]
fn the_use_stamp_is_written_once_per_interval_per_token() {
    let throttle = UseThrottle::new();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let start = Instant::now();
    assert!(throttle.should_record(a, start));
    assert!(!throttle.should_record(a, start));
    assert!(!throttle.should_record(a, start + USE_RECORD_INTERVAL.saturating_sub(Duration::from_secs(1))));
    assert!(throttle.should_record(b, start), "another token is independent");
    assert!(throttle.should_record(a, start + USE_RECORD_INTERVAL));
    assert!(!throttle.should_record(a, start + USE_RECORD_INTERVAL + Duration::from_secs(1)));
}

fn state_for(db: sea_orm::DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("delegated-unit-test-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

#[tokio::test]
async fn resolvers_find_the_workspace_and_project_of_each_kind_of_target() {
    let scratch = oauth_scratch_or_skip!("deleg_resolvers", Start::Complete);
    let db = &scratch.db;
    let one = seed_tenant(db, "r1").await;
    let two = seed_tenant(db, "r2").await;
    let issue = seed_work_item(db, &one, "an item").await;
    let other_issue = seed_work_item(db, &two, "another").await;
    let grantee = Grantee {
        user_id: one.member_id,
        workspace_id: one.workspace_id,
    };
    let id = |name: &'static str, value: Uuid| params(&[(name, value.to_string())]);
    let project = |workspace_id, project| {
        Some(Target {
            workspace_id,
            project: ProjectRef::Project(project),
        })
    };

    assert_eq!(
        resolve_target(db, Resolver::Workspace("w"), &id("w", two.workspace_id), grantee)
            .await
            .expect("ok"),
        Some(Target {
            workspace_id: two.workspace_id,
            project: ProjectRef::NotProjectBound
        })
    );
    assert_eq!(
        resolve_target(db, Resolver::Project("p"), &id("p", one.project_id), grantee)
            .await
            .expect("ok"),
        project(one.workspace_id, one.project_id)
    );
    assert_eq!(
        resolve_target(db, Resolver::Project("p"), &id("p", two.project_id), grantee)
            .await
            .expect("ok"),
        project(two.workspace_id, two.project_id),
        "the resolver reports where the object is; the caller decides whether it is reachable"
    );
    assert_eq!(
        resolve_target(db, Resolver::Project("p"), &id("p", Uuid::new_v4()), grantee)
            .await
            .expect("ok"),
        None
    );
    assert_eq!(
        resolve_target(
            db,
            Resolver::Project("p"),
            &params(&[("p", "not-a-uuid".to_string())]),
            grantee
        )
        .await
        .expect("ok"),
        None
    );
    assert_eq!(
        resolve_target(db, Resolver::Issue("i"), &id("i", issue), grantee)
            .await
            .expect("ok"),
        project(one.workspace_id, one.project_id)
    );
    assert_eq!(
        resolve_target(db, Resolver::Issue("i"), &id("i", other_issue), grantee)
            .await
            .expect("ok"),
        project(two.workspace_id, two.project_id)
    );
    for kind in [Resolver::SelfOnly, Resolver::GrantSelf] {
        assert_eq!(
            resolve_target(db, kind, &[], grantee).await.expect("ok"),
            Some(Target {
                workspace_id: one.workspace_id,
                project: ProjectRef::NotProjectBound
            })
        );
    }

    // By identifier: only the grant's own workspace is searched.
    let sequence = |issue: Uuid| async move {
        use sea_orm::{ConnectionTrait, DbBackend, Statement};
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT sequence_number::bigint AS n FROM work_items WHERE id = $1",
            vec![issue.into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("number")
    };
    let identifier = format!("Pr1-{}", sequence(issue).await);
    assert_eq!(
        resolve_target(
            db,
            Resolver::IssueIdentifier("x"),
            &params(&[("x", identifier.clone())]),
            grantee
        )
        .await
        .expect("ok"),
        project(one.workspace_id, one.project_id),
        "the key matches case-insensitively"
    );
    let foreign = format!("Pr2-{}", sequence(other_issue).await);
    assert_eq!(
        resolve_target(db, Resolver::IssueIdentifier("x"), &params(&[("x", foreign)]), grantee)
            .await
            .expect("ok"),
        None,
        "another workspace's identifier does not resolve"
    );

    // Notifications: the user's own, with a workspace.
    let make = |user: Uuid, workspace: Option<Uuid>, project: Option<Uuid>, issue: Option<Uuid>| async move {
        let id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, \
             related_project_id, related_issue_id) \
             VALUES ($1, $2, $3, 'x', 'x', '{}'::jsonb, 't', 'c', $4, $5)",
            vec![id.into(), user.into(), workspace.into(), project.into(), issue.into()],
        )
        .await;
        id
    };
    let own = make(one.member_id, Some(one.workspace_id), Some(one.project_id), None).await;
    let via_issue = make(one.member_id, Some(one.workspace_id), None, Some(issue)).await;
    let bare = make(one.member_id, Some(one.workspace_id), None, None).await;
    let unattributed = make(one.member_id, None, Some(one.project_id), None).await;
    let others = make(two.member_id, Some(two.workspace_id), Some(two.project_id), None).await;
    for (notification, expected) in [
        (own, project(one.workspace_id, one.project_id)),
        (via_issue, project(one.workspace_id, one.project_id)),
        (
            bare,
            Some(Target {
                workspace_id: one.workspace_id,
                project: ProjectRef::Unresolvable,
            }),
        ),
        (unattributed, None),
        (others, None),
        (Uuid::new_v4(), None),
    ] {
        assert_eq!(
            resolve_target(db, Resolver::Notification("n"), &id("n", notification), grantee)
                .await
                .expect("ok"),
            expected,
            "{notification}"
        );
    }
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_delegated_user_is_a_member_of_the_grants_workspace_whatever_their_stored_role() {
    let scratch = oauth_scratch_or_skip!("deleg_role_cap", Start::Complete);
    let db = &scratch.db;
    let one = seed_tenant(db, "c1").await;
    let two = seed_tenant(db, "c2").await;
    // The user owns workspace one and administers workspace two.
    exec(
        db,
        "UPDATE workspace_members SET role = 'owner' WHERE workspace_id = $1 AND user_id = $2",
        vec![one.workspace_id.into(), one.member_id.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'admin')",
        vec![two.workspace_id.into(), one.member_id.into()],
    )
    .await;
    let state = state_for(db.clone());
    let claims = JwtClaims {
        sub: one.member_id.to_string(),
        email: "u@example.test".to_string(),
        token_type: platform::auth::TokenType::Access,
        iat: 0,
        exp: 0,
        sv: 0,
    };

    // Not delegated: the stored role, as before.
    let (_, role, is_bot) = require_workspace_access_from_auth(&state, &claims, None, one.workspace_id)
        .await
        .expect("a member");
    assert_eq!((role.as_str(), is_bot), ("owner", false));

    // Delegated: `member`, in the grant's workspace; any other workspace is refused, even one the
    // user administers; a removed member gets nothing.
    let delegated = context(one.member_id, one.workspace_id, ProjectScope::All);
    let (actor, role, is_bot) = with_context(delegated.clone(), async {
        require_workspace_access_from_auth(&state, &claims, None, one.workspace_id).await
    })
    .await
    .expect("a member");
    assert_eq!((actor, role.as_str(), is_bot), (one.member_id, "member", false));
    let other = with_context(delegated.clone(), async {
        require_workspace_access_from_auth(&state, &claims, None, two.workspace_id).await
    })
    .await;
    assert!(other.is_err(), "the grant's workspace is the only workspace");
    exec(
        db,
        "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![one.workspace_id.into(), one.member_id.into()],
    )
    .await;
    let removed = with_context(delegated, async {
        require_workspace_access_from_auth(&state, &claims, None, one.workspace_id).await
    })
    .await;
    assert!(removed.is_err(), "membership is read again on every call");
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_dispatch_guard_refuses_bots_only_and_only_for_delegated_requests() {
    let scratch = oauth_scratch_or_skip!("deleg_dispatch_guard", Start::Complete);
    let db = &scratch.db;
    let one = seed_tenant(db, "d1").await;
    let bot = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
         VALUES ($1, $2, '!', 'bot', 'user', true, 'bot')",
        vec![bot.into(), format!("{bot}@example.test").into()],
    )
    .await;
    // Not delegated: nothing is refused.
    refuse_agent_dispatch(db, &[bot])
        .await
        .expect("an ordinary request may name an agent");

    let delegated = context(one.member_id, one.workspace_id, ProjectScope::All);
    with_context(delegated.clone(), async {
        refuse_agent_dispatch(db, &[]).await.expect("nobody named");
        refuse_agent_dispatch(db, &[one.member_id])
            .await
            .expect("a person is fine");
        assert!(refuse_agent_dispatch(db, &[bot]).await.is_err());
        assert!(
            refuse_agent_dispatch(db, &[one.member_id, bot]).await.is_err(),
            "one agent among people"
        );
        assert!(
            refuse_agent_dispatch(db, &[Uuid::new_v4()]).await.is_ok(),
            "an unknown id is not an agent"
        );
    })
    .await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_central_guard_refuses_every_agent_task_creation_under_a_delegated_request() {
    use crate::services::ai_task_service::{
        CreateAiTaskInput, DispatchOrigin, create_ai_task, queue_vote_requested_tasks_for_project,
    };
    let scratch = oauth_scratch_or_skip!("deleg_central_guard", Start::Complete);
    let db = &scratch.db;
    let one = seed_tenant(db, "central").await;
    let bot = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
         VALUES ($1, $2, '!', 'bot', 'user', true, 'bot')",
        vec![bot.into(), format!("{bot}@example.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO ai_participants (id, project_id, name, registered_by) \
         VALUES ($1, $2, 'central bot', $3)",
        vec![bot.to_string().into(), one.project_id.into(), one.member_id.into()],
    )
    .await;
    let tasks = || crate::oauth::test_support::count(db, "SELECT count(*) AS n FROM ai_tasks", vec![]);
    let input = |origin: DispatchOrigin, key: &str| CreateAiTaskInput {
        origin,
        project_id: one.project_id,
        ai_participant_id: bot,
        actor_id: Some(one.member_id),
        task_type: "issue_assigned".to_owned(),
        reference_type: None,
        reference_id: None,
        priority: 0,
        payload: serde_json::json!({}),
        idempotency_key: Some(key.to_owned()),
        max_attempts: 3,
    };

    // An ordinary request and the server itself create tasks.
    assert!(
        create_ai_task(db, input(DispatchOrigin::Request, "plain"))
            .await
            .expect("an ordinary request may create a task")
            .is_some()
    );
    assert!(
        create_ai_task(db, input(DispatchOrigin::System, "system"))
            .await
            .expect("the server may create a task")
            .is_some()
    );
    let before = tasks().await;

    let delegated = context(one.member_id, one.workspace_id, ProjectScope::All);
    with_context(delegated, async {
        // Every way in is refused before anything is written: the plain call, the helper that
        // fans a vote request out to the project's agents, and a call from a spawned task that
        // says whom it acts for.
        assert!(create_ai_task(db, input(DispatchOrigin::Request, "d1")).await.is_err());
        assert!(
            create_ai_task(db, input(DispatchOrigin::Delegated, "d2"))
                .await
                .is_err()
        );
        assert!(
            queue_vote_requested_tasks_for_project(
                db,
                DispatchOrigin::Request,
                one.project_id,
                "proposal-1",
                serde_json::json!({})
            )
            .await
            .is_err()
        );
        let owned = db.clone();
        let project = one.project_id;
        let spawned = tokio::spawn(async move {
            create_ai_task(
                &owned,
                CreateAiTaskInput {
                    origin: DispatchOrigin::Delegated,
                    project_id: project,
                    ai_participant_id: bot,
                    actor_id: None,
                    task_type: "issue_assigned".to_owned(),
                    reference_type: None,
                    reference_id: None,
                    priority: 0,
                    payload: serde_json::json!({}),
                    idempotency_key: Some("d3".to_owned()),
                    max_attempts: 3,
                },
            )
            .await
        });
        assert!(
            spawned.await.expect("task").is_err(),
            "an explicit origin crosses the spawn"
        );
        // The server's own work is not a delegated request, even when it runs inside one's scope.
        assert!(
            create_ai_task(db, input(DispatchOrigin::System, "system-inside"))
                .await
                .expect("system origin")
                .is_some()
        );
    })
    .await;
    assert_eq!(tasks().await, before + 1, "only the system task was created");
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_spawned_task_does_not_inherit_the_delegation_so_it_must_say_where_it_acts_for() {
    let delegated = context(Uuid::new_v4(), Uuid::new_v4(), ProjectScope::All);
    with_context(delegated, async {
        assert!(super::delegated::current().is_some());
        let inside_spawn = tokio::spawn(async { super::delegated::current().is_some() })
            .await
            .expect("task");
        assert!(!inside_spawn, "the task local is not inherited by a spawned task");
    })
    .await;
}
