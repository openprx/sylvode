//! Races between the token endpoint or the consent decision and everything that ends a grant.
//!
//! Each round runs the racing requests at once against PostgreSQL (the requests are spawned on a
//! multi-thread runtime, every one on its own pooled connection) and then checks the end state
//! from the rows: whichever of them committed first, the revoking change leaves no grant active
//! and no token row live. A deadlock between the participants would surface as a 500 (there is
//! no retry any more), so the status checks double as the deadlock check.

#![allow(clippy::enum_variant_names, clippy::excessive_nesting)]

use super::*;

/// The changes that end grants, each through its real handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    UserDisconnect,
    WorkspaceAdminDisconnect,
    SwitchOff,
    ClientDisable,
    MemberRemoval,
    PasswordChange,
}

const ALL_CHANGES: [Change; 6] = [
    Change::UserDisconnect,
    Change::WorkspaceAdminDisconnect,
    Change::SwitchOff,
    Change::ClientDisable,
    Change::MemberRemoval,
    Change::PasswordChange,
];

impl Change {
    const fn reason(self) -> &'static str {
        match self {
            Self::UserDisconnect => "user_revoked",
            Self::WorkspaceAdminDisconnect => "workspace_admin_revoked",
            Self::SwitchOff => "workspace_disabled",
            Self::ClientDisable => "client_disabled",
            Self::MemberRemoval => "membership_removed",
            Self::PasswordChange => "password_changed",
        }
    }

    /// The request that makes the change. `grant` is only used by the two disconnects.
    fn request(self, h: &Harness, user: &Person, grant: Option<Uuid>) -> Request<Body> {
        let ws = h.tenant.workspace_id;
        match self {
            Self::UserDisconnect => request(
                Method::DELETE,
                &apps_path(grant.expect("a grant")),
                Some(&user.bearer),
                &[],
                None,
            ),
            Self::WorkspaceAdminDisconnect => request(
                Method::DELETE,
                &ws_app_path(ws, grant.expect("a grant")),
                Some(&h.ws_admin.bearer),
                &[],
                None,
            ),
            Self::SwitchOff => request(
                Method::PUT,
                &settings_path(ws),
                Some(&h.ws_admin.bearer),
                &[],
                Some(&json!({"allow_connected_apps": false})),
            ),
            Self::ClientDisable => request(
                Method::PATCH,
                "/api/v1/admin/oauth-clients/ramflux",
                Some(&h.inst_admin.bearer),
                &[],
                Some(&json!({"status": "disabled"})),
            ),
            Self::MemberRemoval => request(
                Method::DELETE,
                &format!("/api/v1/workspaces/{ws}/members/{}", user.id),
                Some(&h.ws_admin.bearer),
                &[],
                None,
            ),
            Self::PasswordChange => request(
                Method::PUT,
                "/api/v1/auth/me/password",
                Some(&user.bearer),
                &[],
                Some(&json!({"current_password": "old-password-1", "new_password": "new-password-2"})),
            ),
        }
    }

    /// Puts back what the change switched, so the next round can run.
    async fn undo(self, h: &Harness) {
        match self {
            Self::SwitchOff => {
                h.call(
                    Method::PUT,
                    &settings_path(h.tenant.workspace_id),
                    Some(&h.ws_admin.bearer),
                    &[],
                    Some(&json!({"allow_connected_apps": true})),
                )
                .await
                .assert_ok_data();
            }
            Self::ClientDisable => {
                h.patch_client(json!("active"), "ramflux").await.assert_ok_data();
            }
            _ => {}
        }
    }
}

impl Reply {
    #[track_caller]
    fn assert_ok_data(&self) {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
    }
}

async fn clear_rate_hits(h: &Harness) {
    exec(h.db(), "DELETE FROM oauth_rate_hits", vec![]).await;
}

async fn fresh_user(h: &Harness, change: Change) -> Person {
    let user = h.user_in_both().await;
    if change == Change::PasswordChange {
        set_password(h, user.id, "old-password-1").await;
    }
    user
}

/// Every grant of `user` is revoked and not one token row of theirs is live.
async fn assert_nothing_live(h: &Harness, user: Uuid, context: &str) {
    assert_eq!(
        count(
            h.db(),
            "SELECT count(*) AS n FROM oauth_grants WHERE user_id = $1 AND status = 'active'",
            vec![user.into()]
        )
        .await,
        0,
        "{context}: a grant is still active after the change committed"
    );
    assert_eq!(
        count(
            h.db(),
            "SELECT count(*) AS n FROM oauth_tokens t JOIN oauth_grants g ON g.id = t.grant_id \
             WHERE g.user_id = $1 AND t.revoked_at IS NULL",
            vec![user.into()]
        )
        .await,
        0,
        "{context}: a token of the user is still usable after the change committed"
    );
}

fn redeem_request(code: &str) -> Request<Body> {
    form_request(
        "/api/v1/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT),
            ("client_id", "ramflux"),
            ("code_verifier", VERIFIER),
            ("resource", API_RES),
        ],
    )
}

fn refresh_request(token: &str) -> Request<Body> {
    form_request(
        "/api/v1/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", "ramflux"),
            ("resource", API_RES),
        ],
    )
}

async fn run_all(h: &Harness, round: u32, requests: Vec<Request<Body>>) -> Vec<Reply> {
    let tasks: Vec<_> = requests
        .into_iter()
        .enumerate()
        .map(|(index, req)| {
            let router = h.router.clone();
            tokio::spawn(async move {
                // Vary who starts first.
                for _ in 0..(round as usize + index) % 3 {
                    tokio::task::yield_now().await;
                }
                send_on(router, req).await
            })
        })
        .collect();
    let mut replies = Vec::new();
    for task in tasks {
        replies.push(task.await.expect("task"));
    }
    replies
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redemption_or_refresh_racing_any_revocation_leaves_no_usable_token_and_no_active_grant() {
    let scratch = oauth_scratch_or_skip!("races_token", Start::Complete);
    let h = Harness::new(scratch).await;
    let ws = h.tenant.workspace_id;
    let mut issued = 0_u32;
    for round in 0..12_u32 {
        for change in ALL_CHANGES {
            let context = format!("round {round}, {change:?}");
            let user = fresh_user(&h, change).await;
            let grant = seed_grant(h.db(), user.id, ws, "all", &READ_SCOPES).await;
            seed_access_token(h.db(), grant, "api", &["projects:read"]).await;
            let code = seed_code(h.db(), grant).await;
            let refresh = seed_refresh_token(h.db(), grant).await;
            clear_rate_hits(&h).await;

            let replies = run_all(
                &h,
                round,
                vec![
                    redeem_request(&code),
                    refresh_request(&refresh),
                    change.request(&h, &user, Some(grant)),
                ],
            )
            .await;
            let (redeemed, rotated, changed) = (&replies[0], &replies[1], &replies[2]);
            assert_eq!(changed.status, StatusCode::OK, "{context}: {}", changed.text());
            for reply in [redeemed, rotated] {
                assert!(
                    reply.status == StatusCode::OK || reply.status.is_client_error(),
                    "{context}: the token endpoint failed instead of answering: {} {}",
                    reply.status,
                    reply.text()
                );
                if reply.status == StatusCode::OK {
                    issued += 1;
                }
            }

            let (status, reason, _) = h.grant_state(grant).await;
            assert_eq!(status, "revoked", "{context}");
            assert_eq!(reason.as_deref(), Some(change.reason()), "{context}");
            assert_nothing_live(&h, user.id, &context).await;
            assert_eq!(
                h.audit("grant_revoked", grant).await.len(),
                1,
                "{context}: exactly one revocation audit row"
            );
            // What the endpoint issued before the change is dead.
            for reply in [redeemed, rotated] {
                if reply.status == StatusCode::OK {
                    let body = reply.json();
                    let access = body["access_token"].as_str().expect("access token");
                    assert!(
                        validate_access_token(h.db(), access, AudienceRule::Api(ForwardCredential::absent()))
                            .await
                            .is_err(),
                        "{context}: an access token issued around the change still validates"
                    );
                    if let Some(next) = body["refresh_token"].as_str() {
                        clear_rate_hits(&h).await;
                        let again = h.send(refresh_request(next)).await;
                        assert!(
                            again.status.is_client_error(),
                            "{context}: a refresh token survived: {}",
                            again.text()
                        );
                    }
                }
            }
            change.undo(&h).await;
        }
    }
    eprintln!("token races finished; {issued} token responses were issued before a change");
    h.finish().await;
}

/// Several revocations of the same grant at once, with refreshes and a redemption: the lock order
/// never makes any of them wait in a circle, so none fails. The second phase refreshes with the
/// token the first refresh issued, which is the case where a revocation that waited on the old
/// token meets the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_revocations_and_refreshes_of_one_grant_never_deadlock() {
    let scratch = oauth_scratch_or_skip!("races_deadlock", Start::Complete);
    let h = Harness::new(scratch).await;
    let ws = h.tenant.workspace_id;
    for round in 0..40_u32 {
        let user = h.user_in_both().await;
        let grant = seed_grant(h.db(), user.id, ws, "all", &READ_SCOPES).await;
        seed_access_token(h.db(), grant, "api", &["projects:read"]).await;
        let code = seed_code(h.db(), grant).await;
        let refresh = seed_refresh_token(h.db(), grant).await;
        clear_rate_hits(&h).await;
        let phase = if round % 2 == 1 {
            // Rotate once first and race the new refresh token instead of the seeded one.
            let rotated = h.send(refresh_request(&refresh)).await;
            assert!(rotated.status.is_success(), "round {round}: {}", rotated.text());
            let next = rotated.json()["refresh_token"]
                .as_str()
                .expect("refresh token")
                .to_owned();
            clear_rate_hits(&h).await;
            vec![
                refresh_request(&next),
                refresh_request(&next),
                Change::UserDisconnect.request(&h, &user, Some(grant)),
                Change::WorkspaceAdminDisconnect.request(&h, &user, Some(grant)),
                Change::MemberRemoval.request(&h, &user, Some(grant)),
            ]
        } else {
            vec![
                refresh_request(&refresh),
                redeem_request(&code),
                Change::UserDisconnect.request(&h, &user, Some(grant)),
                Change::WorkspaceAdminDisconnect.request(&h, &user, Some(grant)),
                Change::MemberRemoval.request(&h, &user, Some(grant)),
                refresh_request(&refresh),
            ]
        };
        let replies = run_all(&h, round, phase).await;
        for (index, reply) in replies.iter().enumerate() {
            assert!(
                reply.status.as_u16() < 500,
                "round {round}, request {index}: {} {}",
                reply.status,
                reply.text()
            );
        }
        // At least one of the three revoking requests ended the grant; the others found it ended.
        let (status, _, _) = h.grant_state(grant).await;
        assert_eq!(status, "revoked", "round {round}");
        assert_nothing_live(&h, user.id, &format!("round {round}")).await;
        assert_eq!(h.audit("grant_revoked", grant).await.len(), 1, "round {round}");
    }
    h.finish().await;
}

/// The consent decision races each change that would end the grant it creates. Either the
/// decision is refused, or the grant it created is revoked by the change: never an active grant
/// after the change committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consent_racing_a_revoking_change_never_leaves_an_active_grant() {
    let scratch = oauth_scratch_or_skip!("races_consent", Start::Complete);
    let h = Harness::new(scratch).await;
    let ws = h.tenant.workspace_id;
    let mut granted = 0_u32;
    let mut refused = 0_u32;
    for round in 0..14_u32 {
        for change in [
            Change::SwitchOff,
            Change::ClientDisable,
            Change::MemberRemoval,
            Change::PasswordChange,
        ] {
            let context = format!("round {round}, {change:?}");
            let user = fresh_user(&h, change).await;
            clear_rate_hits(&h).await;
            let begin = json!({
                "response_type": "code", "client_id": "ramflux", "redirect_uri": REDIRECT, "scope": SCOPES,
                "state": STATE, "code_challenge": s256_challenge(VERIFIER), "code_challenge_method": "S256",
                "resource": [API_RES, MCP_RES],
            });
            let data = h
                .call(
                    Method::POST,
                    "/api/v1/oauth/authorization-requests",
                    Some(&user.bearer),
                    &[],
                    Some(&begin),
                )
                .await
                .data();
            let request_id = data["request_id"].as_str().expect("request id").to_owned();
            let consent = data["consent_token"].as_str().expect("consent token").to_owned();
            let decision = request(
                Method::POST,
                &format!("/api/v1/oauth/authorization-requests/{request_id}/decision"),
                Some(&user.bearer),
                &[("x-sylvode-consent", &consent)],
                Some(&json!({
                    "decision": "allow",
                    "workspace_id": ws,
                    "project_mode": "all",
                    "scopes": SCOPES.split(' ').collect::<Vec<_>>(),
                })),
            );
            let replies = run_all(&h, round, vec![decision, change.request(&h, &user, None)]).await;
            let (decided, changed) = (&replies[0], &replies[1]);
            assert_eq!(changed.status, StatusCode::OK, "{context}: {}", changed.text());
            assert!(
                decided.status.as_u16() < 500,
                "{context}: the decision failed: {} {}",
                decided.status,
                decided.text()
            );
            assert_nothing_live(&h, user.id, &context).await;
            let grants = count(
                h.db(),
                "SELECT count(*) AS n FROM oauth_grants WHERE user_id = $1",
                vec![user.id.into()],
            )
            .await;
            if decided.status == StatusCode::OK {
                granted += 1;
                assert_eq!(grants, 1, "{context}: a granted consent made one grant");
                // The code that consent returned is useless: its grant is revoked.
                let code = decided
                    .text()
                    .split("code=")
                    .nth(1)
                    .and_then(|rest| rest.split(['&', '"', '\\']).next().map(str::to_owned))
                    .unwrap_or_else(|| panic!("{context}: no code in {}", decided.text()));
                clear_rate_hits(&h).await;
                let redeemed = h.send(redeem_request(&code)).await;
                assert!(
                    redeemed.status.is_client_error(),
                    "{context}: a code of a revoked grant was redeemed: {}",
                    redeemed.text()
                );
            } else {
                refused += 1;
                assert_eq!(grants, 0, "{context}: a refused consent left a grant behind");
            }
            change.undo(&h).await;
        }
    }
    eprintln!("consent races finished; {granted} consents committed before their change, {refused} refused");
    h.finish().await;
}

/// Deleting a workspace removes its grants by cascade (grant rows first, then their tokens), the
/// same direction as every other path, so a refresh in flight never deadlocks with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_workspace_racing_a_refresh_or_redemption_never_deadlocks() {
    let scratch = oauth_scratch_or_skip!("races_ws_delete", Start::Complete);
    let h = Harness::new(scratch).await;
    for round in 0..25_u32 {
        let tenant = seed_tenant(h.db(), "ws-delete").await;
        exec(
            h.db(),
            "UPDATE workspace_members SET role = 'owner' WHERE workspace_id = $1 AND user_id = $2",
            vec![tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let owner = person(tenant.member_id);
        let grant = seed_grant(h.db(), owner.id, tenant.workspace_id, "all", &READ_SCOPES).await;
        let code = seed_code(h.db(), grant).await;
        let refresh = seed_refresh_token(h.db(), grant).await;
        clear_rate_hits(&h).await;
        let replies = run_all(
            &h,
            round,
            vec![
                refresh_request(&refresh),
                redeem_request(&code),
                request(
                    Method::DELETE,
                    &format!("/api/v1/workspaces/{}", tenant.workspace_id),
                    Some(&owner.bearer),
                    &[],
                    None,
                ),
            ],
        )
        .await;
        for (index, reply) in replies.iter().enumerate() {
            assert!(
                reply.status.as_u16() < 500,
                "round {round}, request {index}: {} {}",
                reply.status,
                reply.text()
            );
        }
        assert_eq!(
            replies[2].status,
            StatusCode::OK,
            "round {round}: {}",
            replies[2].text()
        );
        assert_eq!(
            count(
                h.db(),
                "SELECT count(*) AS n FROM oauth_grants WHERE workspace_id = $1",
                vec![tenant.workspace_id.into()]
            )
            .await,
            0,
            "round {round}: the grants went with the workspace"
        );
    }
    h.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Deterministic orderings: the locks themselves
// ---------------------------------------------------------------------------------------------

/// How long a request that must be waiting on a lock is given to prove it is not finishing.
const WAIT_PROOF: Duration = Duration::from_millis(400);

/// Runs `request` on a spawned task and shows that it has not finished while `holder` still holds
/// its uncommitted change; then commits the holder and returns the reply.
async fn must_wait_for(h: &Harness, holder: sea_orm::DatabaseTransaction, request: Request<Body>, what: &str) -> Reply {
    let router = h.router.clone();
    let task = tokio::spawn(async move { send_on(router, request).await });
    tokio::time::sleep(WAIT_PROOF).await;
    assert!(
        !task.is_finished(),
        "{what}: the request finished while the change it must wait for was uncommitted"
    );
    holder.commit().await.expect("the holder commits");
    task.await.expect("task")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_token_endpoint_waits_for_a_revocation_that_holds_the_grant() {
    use sea_orm::TransactionTrait;
    let scratch = oauth_scratch_or_skip!("races_grant_lock", Start::Complete);
    let h = Harness::new(scratch).await;
    let ws = h.tenant.workspace_id;
    for kind in ["code", "refresh"] {
        let user = h.user_in_both().await;
        let grant = seed_grant(h.db(), user.id, ws, "all", &READ_SCOPES).await;
        let code = seed_code(h.db(), grant).await;
        let refresh = seed_refresh_token(h.db(), grant).await;
        clear_rate_hits(&h).await;
        let before = count(
            h.db(),
            "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1",
            vec![grant.into()],
        )
        .await;
        // A revocation that has locked and ended the grant row but has not committed.
        let holder = h.db().begin().await.expect("begin");
        exec(
            &holder,
            "UPDATE oauth_grants SET status = 'revoked', revoked_at = now(), revoke_reason = 'user_revoked' \
             WHERE id = $1",
            vec![grant.into()],
        )
        .await;
        let req = if kind == "code" {
            redeem_request(&code)
        } else {
            refresh_request(&refresh)
        };
        let reply = must_wait_for(&h, holder, req, kind).await;
        assert!(
            reply.status.is_client_error(),
            "{kind}: tokens were issued for a grant revoked by a concurrent change: {} {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            count(
                h.db(),
                "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1",
                vec![grant.into()]
            )
            .await,
            before,
            "{kind}: a token row was inserted for the revoked grant"
        );
    }
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consent_waits_for_the_change_that_would_revoke_it() {
    use sea_orm::TransactionTrait;
    let scratch = oauth_scratch_or_skip!("races_consent_locks", Start::Complete);
    let h = Harness::new(scratch).await;
    let ws = h.tenant.workspace_id;
    let begin = |user: &Person| {
        let begin = json!({
            "response_type": "code", "client_id": "ramflux", "redirect_uri": REDIRECT, "scope": SCOPES,
            "state": STATE, "code_challenge": s256_challenge(VERIFIER), "code_challenge_method": "S256",
            "resource": [API_RES, MCP_RES],
        });
        request(
            Method::POST,
            "/api/v1/oauth/authorization-requests",
            Some(&user.bearer),
            &[],
            Some(&begin),
        )
    };
    // (name, statement an in-flight change has executed and not committed)
    let holders: [(&str, &str); 4] = [
        (
            "membership removal",
            "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        ),
        (
            "workspace switch-off",
            "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) VALUES ($1, false) \
             ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
        ),
        (
            "client disable",
            "UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux' AND $1::uuid IS NOT NULL",
        ),
        (
            "workspace epoch (member removal)",
            "SELECT id FROM workspaces WHERE id = $1 FOR UPDATE",
        ),
    ];
    for (name, statement) in holders {
        let user = h.user_in_both().await;
        clear_rate_hits(&h).await;
        let started = h.send(begin(&user)).await.data();
        let request_id = started["request_id"].as_str().expect("request id").to_owned();
        let consent = started["consent_token"].as_str().expect("consent token").to_owned();
        let decision = request(
            Method::POST,
            &format!("/api/v1/oauth/authorization-requests/{request_id}/decision"),
            Some(&user.bearer),
            &[("x-sylvode-consent", &consent)],
            Some(&json!({
                "decision": "allow", "workspace_id": ws, "project_mode": "all",
                "scopes": SCOPES.split(' ').collect::<Vec<_>>(),
            })),
        );
        let holder = h.db().begin().await.expect("begin");
        let values: Vec<sea_orm::Value> = if name == "membership removal" {
            vec![ws.into(), user.id.into()]
        } else {
            vec![ws.into()]
        };
        exec(&holder, statement, values).await;
        let reply = must_wait_for(&h, holder, decision, name).await;
        // The change committed first. Switch-off, membership removal and client disable leave the
        // consent refused; the epoch fence alone changes nothing the consent depends on, so there
        // the consent goes through after it.
        let grants = count(
            h.db(),
            "SELECT count(*) AS n FROM oauth_grants WHERE user_id = $1 AND status = 'active'",
            vec![user.id.into()],
        )
        .await;
        if name == "workspace epoch (member removal)" {
            assert_eq!(reply.status, StatusCode::OK, "{name}: {}", reply.text());
            assert_eq!(grants, 1, "{name}");
        } else {
            assert!(
                reply.status.is_client_error(),
                "{name}: a consent committed after the change that forbids it: {} {}",
                reply.status,
                reply.text()
            );
            assert_eq!(grants, 0, "{name}: a grant was created");
        }
        // Put back what the case switched, for the next one.
        match name {
            "client disable" => h.patch_client(json!("active"), "ramflux").await.assert_ok_data(),
            "workspace switch-off" => Change::SwitchOff.undo(&h).await,
            _ => {}
        }
    }
    h.finish().await;
}
