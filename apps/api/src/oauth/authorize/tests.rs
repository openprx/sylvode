//! Unit tests of request validation, the consent check, the origin check and error rendering.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

use axum::http::{HeaderMap, HeaderValue, header};
use serde_json::json;

use super::*;
use crate::oauth::token::s256_challenge;

const ISSUER: &str = "https://sv.example.test";
const REDIRECT: &str = "http://127.0.0.1:53682/oauth/sylvode/callback";
const STATE: &str = "state-value-0123456789abcdef";

fn config() -> OAuthConfig {
    let mut config = OAuthConfig::with_defaults(ISSUER);
    config.mcp_resource = Some("https://sv.example.test/mcp".to_owned());
    config
}

fn client() -> ClientRow {
    ClientRow {
        client_id: "ramflux".to_owned(),
        display_name: "Ramflux".to_owned(),
        description: String::new(),
        status: "active".to_owned(),
        redirect_uris: vec![
            "http://127.0.0.1/oauth/sylvode/callback".to_owned(),
            "http://[::1]/oauth/sylvode/callback".to_owned(),
        ],
        allowed_scopes: [
            "projects:read",
            "work_items:read",
            "notifications:read",
            "tasks:read",
            "offline_access",
        ]
        .map(str::to_owned)
        .to_vec(),
        allowed_resources: vec!["api".to_owned(), "mcp".to_owned()],
    }
}

fn valid() -> serde_json::Value {
    json!({
        "response_type": "code",
        "client_id": "ramflux",
        "redirect_uri": REDIRECT,
        "scope": "projects:read work_items:read offline_access",
        "state": STATE,
        "code_challenge": s256_challenge("a-verifier-of-sufficient-length-0123456789"),
        "code_challenge_method": "S256",
        "resource": ["https://sv.example.test/api", "https://sv.example.test/mcp"],
        "device_name": "MacBook",
        "workspace_hint": "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
    })
}

fn run(mutate: impl FnOnce(&mut serde_json::Value)) -> Result<Validated, Rejection> {
    let mut body = valid();
    mutate(&mut body);
    let body: AuthorizeBody = serde_json::from_value(body).expect("body shape");
    validate(&config(), &client(), &body)
}

fn without(key: &'static str) -> impl FnOnce(&mut serde_json::Value) {
    move |body| {
        body.as_object_mut().expect("object").remove(key);
    }
}

fn with(key: &'static str, value: serde_json::Value) -> impl FnOnce(&mut serde_json::Value) {
    move |body| {
        body.as_object_mut().expect("object").insert(key.to_owned(), value);
    }
}

#[track_caller]
fn assert_redirect(result: Result<Validated, Rejection>, error: &str, check: &str) {
    match result {
        Err(Rejection::Redirect {
            error: got_error,
            check: got_check,
            redirect_uri,
            ..
        }) => {
            assert_eq!((got_error, got_check), (error, check));
            assert_eq!(redirect_uri, REDIRECT, "an error goes to the validated URI");
        }
        other => panic!("expected a redirect with {error}/{check}, got {other:?}"),
    }
}

#[track_caller]
fn assert_page(result: Result<Validated, Rejection>, error: &str) {
    match result {
        Err(Rejection::ErrorPage { error: got, .. }) => assert_eq!(got, error),
        other => panic!("expected the error page {error}, got {other:?}"),
    }
}

#[test]
fn a_complete_request_validates() {
    let valid = run(|_| {}).expect("valid");
    assert_eq!(valid.redirect_uri, REDIRECT);
    assert_eq!(valid.state, STATE);
    assert_eq!(valid.resources, [TokenResource::Api, TokenResource::Mcp]);
    assert_eq!(
        valid.scopes.to_scope_string(),
        "projects:read work_items:read offline_access"
    );
    assert_eq!(valid.device_name.as_deref(), Some("MacBook"));
    assert!(valid.workspace_hint.is_some());
    // One resource as a bare string is accepted as well as a list; the optional fields may be absent.
    let single = run(|body| {
        body["resource"] = json!("https://sv.example.test/api");
        body.as_object_mut().unwrap().remove("device_name");
        body.as_object_mut().unwrap().remove("workspace_hint");
    })
    .expect("valid");
    assert_eq!(single.resources, [TokenResource::Api]);
    assert!(single.device_name.is_none() && single.workspace_hint.is_none());
    // The port may be any loopback port and the other loopback literal is registered too.
    assert!(run(with("redirect_uri", json!("http://[::1]:4000/oauth/sylvode/callback"))).is_ok());
}

#[test]
fn client_and_redirect_failures_are_never_redirected() {
    for uri in [
        json!("http://localhost:53682/oauth/sylvode/callback"),
        json!("http://127.0.0.1:80/oauth/sylvode/callback"),
        json!("http://127.0.0.1:53682/other"),
        json!("https://127.0.0.1:53682/oauth/sylvode/callback"),
        json!("http://user@127.0.0.1:53682/oauth/sylvode/callback"),
        json!("http://127.0.0.1:53682/oauth/sylvode/callback?x=1"),
        json!("https://evil.example/cb"),
        json!(""),
    ] {
        assert_page(run(with("redirect_uri", uri.clone())), "redirect_uri_mismatch");
    }
    assert_page(run(without("redirect_uri")), "redirect_uri_mismatch");
    // Even with every other parameter broken, a bad redirect URI wins and nothing is redirected.
    assert_page(
        run(|body| {
            body["redirect_uri"] = json!("https://evil.example/cb");
            body["response_type"] = json!("token");
            body.as_object_mut().unwrap().remove("state");
        }),
        "redirect_uri_mismatch",
    );
}

#[test]
fn each_parameter_failure_redirects_with_its_standard_error() {
    assert_redirect(
        run(with("response_type", json!("token"))),
        "unsupported_response_type",
        "response_type",
    );
    assert_redirect(
        run(without("response_type")),
        "unsupported_response_type",
        "response_type",
    );

    assert_redirect(run(without("code_challenge")), "invalid_request", "code_challenge");
    assert_redirect(
        run(with("code_challenge", json!("short"))),
        "invalid_request",
        "code_challenge",
    );
    assert_redirect(
        run(with("code_challenge", json!("a".repeat(129)))),
        "invalid_request",
        "code_challenge",
    );
    assert_redirect(
        run(with("code_challenge", json!(format!("{}=", "a".repeat(43))))),
        "invalid_request",
        "code_challenge",
    );
    assert_redirect(
        run(with("code_challenge_method", json!("plain"))),
        "invalid_request",
        "code_challenge_method",
    );
    assert_redirect(
        run(with("code_challenge_method", json!("s256"))),
        "invalid_request",
        "code_challenge_method",
    );
    assert_redirect(
        run(without("code_challenge_method")),
        "invalid_request",
        "code_challenge_method",
    );

    assert_redirect(run(without("state")), "invalid_request", "state");
    assert_redirect(run(with("state", json!("too-short"))), "invalid_request", "state");
    assert_redirect(run(with("state", json!("a".repeat(513)))), "invalid_request", "state");
    assert_redirect(
        run(with("state", json!("has a space in it......."))),
        "invalid_request",
        "state",
    );
    assert_redirect(
        run(with("state", json!("non-ascii-state-é-0123456789"))),
        "invalid_request",
        "state",
    );

    assert_redirect(run(without("resource")), "invalid_target", "resource");
    assert_redirect(run(with("resource", json!([]))), "invalid_target", "resource");
    assert_redirect(
        run(with("resource", json!(["https://sv.example.test/other"]))),
        "invalid_target",
        "resource",
    );
    assert_redirect(
        run(with(
            "resource",
            json!(["https://sv.example.test/api", "https://sv.example.test/api"]),
        )),
        "invalid_target",
        "resource",
    );
    assert_redirect(
        run(with("resource", json!(["https://sv.example.test/api/"]))),
        "invalid_target",
        "resource",
    );

    assert_redirect(run(without("scope")), "invalid_scope", "scope");
    assert_redirect(run(with("scope", json!(""))), "invalid_scope", "scope");
    assert_redirect(
        run(with("scope", json!("projects:read bogus"))),
        "invalid_scope",
        "scope",
    );
    assert_redirect(run(with("scope", json!("tasks:dispatch"))), "invalid_scope", "scope");
    assert_redirect(run(with("scope", json!("flow:read"))), "invalid_scope", "scope");
    assert_redirect(
        run(with("scope", json!("projects:read  work_items:read"))),
        "invalid_scope",
        "scope",
    );
    // In the vocabulary but not allowed for the client.
    assert_redirect(
        run(with("scope", json!("projects:read forms:write"))),
        "invalid_scope",
        "scope",
    );

    assert_redirect(
        run(with("device_name", json!("x".repeat(65)))),
        "invalid_request",
        "device_name",
    );
    assert_redirect(run(with("device_name", json!(""))), "invalid_request", "device_name");
    assert_redirect(
        run(with("device_name", json!("bad\u{7}name"))),
        "invalid_request",
        "device_name",
    );
    assert_redirect(
        run(with("workspace_hint", json!("not-a-uuid"))),
        "invalid_request",
        "workspace_hint",
    );
}

#[test]
fn the_check_order_is_the_contract_order() {
    // Response type is checked before the challenge, the challenge before state, state before
    // resource, resource before scope.
    let all_wrong = |body: &mut serde_json::Value| {
        body["response_type"] = json!("token");
        body["code_challenge"] = json!("x");
        body.as_object_mut().unwrap().remove("state");
        body["resource"] = json!([]);
        body["scope"] = json!("bogus");
    };
    assert_redirect(run(all_wrong), "unsupported_response_type", "response_type");
    assert_redirect(
        run(|body| {
            all_wrong(body);
            body["response_type"] = json!("code");
        }),
        "invalid_request",
        "code_challenge",
    );
    assert_redirect(
        run(|body| {
            all_wrong(body);
            body["response_type"] = json!("code");
            body["code_challenge"] = valid()["code_challenge"].clone();
            body["code_challenge_method"] = json!("S256");
        }),
        "invalid_request",
        "state",
    );
    assert_redirect(
        run(|body| {
            all_wrong(body);
            body["response_type"] = json!("code");
            body["code_challenge"] = valid()["code_challenge"].clone();
            body["state"] = json!(STATE);
        }),
        "invalid_target",
        "resource",
    );
}

#[test]
fn the_resource_must_be_allowed_for_the_client_and_configured() {
    let mut only_api = client();
    only_api.allowed_resources = vec!["api".to_owned()];
    let body: AuthorizeBody = serde_json::from_value(valid()).expect("body");
    assert_redirect(validate(&config(), &only_api, &body), "invalid_target", "resource");
    let mut no_mcp = config();
    no_mcp.mcp_resource = None;
    assert_redirect(validate(&no_mcp, &client(), &body), "invalid_target", "resource");
}

#[test]
fn state_is_returned_untouched_and_only_when_it_is_plain() {
    let config = config();
    let rejection = run(with("state", json!("a&b=c d")));
    // Not plain (space), so not echoed.
    let Err(Rejection::Redirect { state, .. }) = &rejection else {
        panic!("expected a redirect");
    };
    assert_eq!(*state, None);

    // A short state is still echoed untouched with the error about its length.
    let rejection = run(with("state", json!("abc&def")));
    let url = error_redirect(&config, &rejection.unwrap_err()).expect("redirect");
    assert_eq!(
        url,
        format!(
            "{REDIRECT}?error=invalid_request&error_description=a%20valid%20state%20is%20required\
             &state=abc%26def&iss=https%3A%2F%2Fsv.example.test"
        )
    );

    // Every error carries iss and the state; none carries a code.
    let rejection = run(with("scope", json!("bogus"))).unwrap_err();
    let url = error_redirect(&config, &rejection).expect("redirect");
    assert!(url.contains(&format!("state={STATE}")), "{url}");
    assert!(url.contains("iss=https%3A%2F%2Fsv.example.test"), "{url}");
    assert!(url.contains("error=invalid_scope"), "{url}");
    assert!(!url.contains("code="), "{url}");
    // The page error has no redirect at all.
    assert!(error_redirect(&config, &run(without("redirect_uri")).unwrap_err()).is_none());
    // A state of 513 characters is not echoed (bounded), whatever its characters.
    let Err(Rejection::Redirect { state, .. }) = run(with("state", json!("a".repeat(513)))) else {
        panic!("expected a redirect");
    };
    assert_eq!(state, None);
}

#[test]
fn the_origin_check_accepts_only_the_issuer_and_same_origin() {
    let check = |origin: Option<&str>, site: Option<&str>| {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        if let Some(site) = site {
            headers.insert("sec-fetch-site", HeaderValue::from_str(site).unwrap());
        }
        check_same_origin(&headers, ISSUER)
    };
    assert!(check(None, None).is_ok(), "non-browser clients send neither");
    assert!(check(Some(ISSUER), Some("same-origin")).is_ok());
    assert!(check(Some(ISSUER), None).is_ok());
    for (origin, site) in [
        (Some("https://evil.example"), None),
        (Some("null"), None),
        (Some("https://sv.example.test:8443"), None),
        (Some("http://sv.example.test"), None),
        (Some("https://sv.example.test/"), None),
        (Some(ISSUER), Some("cross-site")),
        (Some(ISSUER), Some("same-site")),
        (Some(ISSUER), Some("none")),
        (None, Some("cross-site")),
        (None, Some("same-site")),
    ] {
        let error = check(origin, site).expect_err("refused");
        assert_eq!(
            (error.status, error.code),
            (StatusCode::FORBIDDEN, "csrf_origin_mismatch")
        );
    }
}

fn stored(user: Uuid, token: &str) -> StoredRequest {
    StoredRequest {
        id: Uuid::new_v4(),
        client_id: "ramflux".to_owned(),
        redirect_uri: REDIRECT.to_owned(),
        state: STATE.to_owned(),
        code_challenge: "c".repeat(43),
        requested_scopes: vec!["projects:read".to_owned()],
        resources: vec!["api".to_owned()],
        device_name: None,
        workspace_hint: None,
        consent_token_hash: Some(hash_secret(token).as_hex().to_owned()),
        consent_user_id: Some(user),
        status: "pending".to_owned(),
        live: true,
    }
}

#[test]
fn the_consent_token_binds_request_and_user() {
    let user = Uuid::new_v4();
    let session = Session {
        user_id: user,
        name: "A".to_owned(),
        email: "a@example.test".to_owned(),
        version: 0,
    };
    let token = "ab".repeat(32);
    let request = stored(user, &token);
    assert!(consent_matches(&request, Some(&token), &session));
    assert!(!consent_matches(&request, None, &session));
    assert!(!consent_matches(&request, Some(&"cd".repeat(32)), &session));
    assert!(!consent_matches(&request, Some("short"), &session));
    assert!(!consent_matches(&request, Some(&token.to_uppercase()), &session));
    let other = Session {
        user_id: Uuid::new_v4(),
        ..session.clone()
    };
    assert!(
        !consent_matches(&request, Some(&token), &other),
        "another user's session"
    );
    let mut unissued = stored(user, &token);
    unissued.consent_token_hash = None;
    assert!(!consent_matches(&unissued, Some(&token), &session));
    let mut unbound = stored(user, &token);
    unbound.consent_user_id = None;
    assert!(!consent_matches(&unbound, Some(&token), &session));
}

fn decision(value: serde_json::Value) -> DecisionBody {
    serde_json::from_value(value).expect("decision body")
}

fn request_with(scopes: &[&str], resources: &[&str]) -> StoredRequest {
    let mut request = stored(Uuid::new_v4(), &"ab".repeat(32));
    request.requested_scopes = scopes.iter().map(|s| (*s).to_owned()).collect();
    request.resources = resources.iter().map(|s| (*s).to_owned()).collect();
    request
}

fn selection_error(request: &StoredRequest, body: serde_json::Value) -> String {
    match parse_selection(request, &decision(body)) {
        Err(fail) => format!("{}:{}", fail.error.status.as_u16(), fail.error.message),
        Ok(_) => "accepted".to_owned(),
    }
}

#[test]
fn the_selection_is_checked_against_the_stored_request() {
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    let request = request_with(
        &["projects:read", "work_items:read", "work_items:write", "offline_access"],
        &["api", "mcp"],
    );
    let ok = |body: serde_json::Value| parse_selection(&request, &decision(body)).ok();

    let all = ok(json!({"decision":"allow","workspace_id":workspace,"project_mode":"all",
        "scopes":["projects:read","work_items:read","offline_access"]}))
    .expect("accepted");
    assert!(all.all_projects && all.project_ids.is_empty());
    assert!(
        !all.scopes.contains(Scope::WorkItemsWrite),
        "the optional scope was unticked"
    );

    let selected = ok(
        json!({"decision":"allow","workspace_id":workspace,"project_mode":"selected",
        "project_ids":[project, project],
        "scopes":["projects:read","work_items:read","work_items:write","offline_access"]}),
    )
    .expect("accepted");
    assert_eq!(selected.project_ids, [project], "duplicates collapse");
    assert!(selected.scopes.contains(Scope::WorkItemsWrite));

    let base = |extra: serde_json::Value| {
        let mut body = json!({"decision":"allow","workspace_id":workspace,"project_mode":"all",
            "scopes":["projects:read","work_items:read","offline_access"]});
        for (key, value) in extra.as_object().expect("object") {
            body[key] = value.clone();
        }
        body
    };
    for (label, body) in [
        ("no workspace", base(json!({"workspace_id": null}))),
        ("no mode", base(json!({"project_mode": null}))),
        ("unknown mode", base(json!({"project_mode": "some"}))),
        ("selected without projects", base(json!({"project_mode": "selected"}))),
        (
            "selected with an empty list",
            base(json!({"project_mode": "selected", "project_ids": []})),
        ),
        ("all with projects", base(json!({"project_ids": [project]}))),
        ("no scopes", base(json!({"scopes": []}))),
        ("missing scopes field", base(json!({"scopes": null}))),
        ("unknown scope", base(json!({"scopes": ["projects:read", "bogus"]}))),
        (
            "a scope that was not requested",
            base(json!({"scopes": ["projects:read","work_items:read","offline_access","tasks:read"]})),
        ),
        (
            "a required scope unticked",
            base(json!({"scopes": ["projects:read", "offline_access"]})),
        ),
        (
            "offline_access unticked",
            base(json!({"scopes": ["projects:read", "work_items:read"]})),
        ),
    ] {
        assert!(ok(body).is_none(), "{label} must be refused");
    }
    let too_many: Vec<Uuid> = (0..=MAX_SELECTED_PROJECTS).map(|_| Uuid::new_v4()).collect();
    assert!(ok(base(json!({"project_mode": "selected", "project_ids": too_many}))).is_none());
    assert_eq!(
        selection_error(&request, base(json!({"scopes": []}))),
        "400:the scopes are not valid"
    );
}

#[test]
fn granted_scopes_must_cover_every_requested_resource() {
    let workspace = Uuid::new_v4();
    // Only offline_access, which no resource supports, would leave the MCP resource empty.
    let request = request_with(&["offline_access"], &["api", "mcp"]);
    assert_eq!(
        selection_error(
            &request,
            json!({"decision":"allow","workspace_id":workspace,"project_mode":"all","scopes":["offline_access"]})
        ),
        "400:the scopes do not cover a requested resource"
    );
    // Scopes the API supports but MCP does not leave the MCP resource empty.
    let request = request_with(&["tasks:read"], &["api", "mcp"]);
    assert_eq!(
        selection_error(
            &request,
            json!({"decision":"allow","workspace_id":workspace,"project_mode":"all","scopes":["tasks:read"]})
        ),
        "400:the scopes do not cover a requested resource"
    );
    let request = request_with(&["tasks:read"], &["api"]);
    assert_eq!(
        selection_error(
            &request,
            json!({"decision":"allow","workspace_id":workspace,"project_mode":"all","scopes":["tasks:read"]})
        ),
        "accepted"
    );
}

#[tokio::test]
async fn failures_render_with_status_envelope_fields_and_no_store() {
    let mut error = FlowError::new(
        StatusCode::CONFLICT,
        "too_many_grants",
        "disconnect an older connection first",
    );
    error.details = Some(json!({"max_active_grants": 10}));
    error.retry_after_secs = Some(7);
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::RETRY_AFTER], "7");
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["code"], 409);
    assert_eq!(body["error_code"], "too_many_grants");
    assert_eq!(body["error"], "too_many_grants");
    assert_eq!(body["message"], "disconnect an older connection first");
    assert_eq!(body["details"]["max_active_grants"], 10);

    let from_oauth: FlowError = OAuthError::rate_limited(5).into();
    assert_eq!(
        (from_oauth.status, from_oauth.code),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited")
    );
    assert_eq!(from_oauth.retry_after_secs, Some(5));
}
