//! Handlers of the public site API `/api/site/v1/{slug}` (contract C3 section 6.6).
//!
//! Every handler runs behind [`super::http::guard`], which has already resolved the site, checked
//! origin and CSRF, the body size and the address rate; the handler resolves the caller from the
//! site cookies, passes the catalogue's route check, and applies the binding projection.

use std::collections::{BTreeSet, HashMap};

use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use platform::app::AppState;
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::http::{Bucket, ClientAddress, HOUR, SiteError, SiteJson, WINDOWS, empty_object, ok, set_cookie};
use super::layouts;
use super::projection::{Audience, effective_read, field_items};
use super::records::{self, Binding, ListRequest, MAX_PER_PAGE};
use super::session::{self, Caller, operator_required};
use super::store::{self, SiteRow};
use crate::catalogue::SiteRole;
use crate::forms::native_create::{CreateActor, NativeRecordCreateRequest, create_record_as, finish_record_creation};
use crate::forms::rules::external::RuleCaller;
use crate::response::PaginatedData;
use crate::routes::form::UpdateRecordRequest;

/// Login attempts per minute, per address and per e-mail.
const LOGINS_PER_MINUTE: u32 = 5;

async fn caller(state: &AppState, site: SiteRow, headers: &HeaderMap, op: &str) -> Result<Caller, SiteError> {
    let caller = Caller::load(state, site, headers).await?;
    caller.admit(op)?;
    Ok(caller)
}

/// `GET /manifest`
pub async fn manifest(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.manifest.read").await?;
    let operator = caller.operator.as_ref();
    let mut pages = Vec::new();
    for page in store::list_pages(&state.db, caller.site.id).await? {
        if page.audience == "operator" && operator.is_none() {
            continue;
        }
        let layout_key = store::load_page_layout(&state.db, &caller.site, &page)
            .await?
            .map(|layout| layout.layout_key)
            .unwrap_or_default();
        pages.push(json!({
            "route": page.route,
            "audience": page.audience,
            "nav": page.nav,
            "layout_key": layout_key,
        }));
    }
    let mut bindings = Vec::new();
    for row in store::list_bindings(&state.db, caller.site.id).await? {
        let Ok(binding) = Binding::load(&state, &caller, &row.binding_key).await else {
            continue;
        };
        let visitor_read = binding
            .projection
            .read(Audience::Visitor)
            .map(|read| effective_read(read, &binding.fields, &binding.form.title_template, &BTreeSet::new()))
            .unwrap_or_default();
        let operator_block = match operator {
            Some(operator) => {
                let access = records::read_access(&state, &caller, &binding).await.ok().flatten();
                let read = access.map(|access| access.read).unwrap_or_default();
                let update = records::operator_excluded(&state, &binding, operator)
                    .await
                    .map_or_else(|_| Vec::new(), |excluded| records::update_fields(&binding, &excluded));
                json!({
                    "read_fields": read.fields,
                    "update_fields": update,
                    "filter_fields": read.filter,
                    "sort_fields": read.sort,
                })
            }
            None => Value::Null,
        };
        bindings.push(json!({
            "key": binding.row.binding_key,
            "visitor": {
                "read_fields": visitor_read.fields,
                "submit_fields": records::submit_fields(&binding),
                "filter_fields": visitor_read.filter,
                "sort_fields": visitor_read.sort,
            },
            "operator": operator_block,
        }));
    }
    let site = &caller.site;
    Ok(ok(
        StatusCode::OK,
        json!({
            "site": {
                "slug": site.slug,
                "title": site.title,
                "theme": site.settings.get("theme").cloned().unwrap_or(Value::Null),
            },
            "operator": operator.map(|operator| json!({
                "user_id": operator.user_id,
                "display_name": operator.display_name,
                "role": operator.role.as_str(),
            })),
            "pages": pages,
            "bindings": bindings,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    route: String,
}

/// `GET /pages?route=`: the page whose route matches the path (a static route before a
/// parameter route), its route parameters and its layout with `binding:<key>` sources.
pub async fn page(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.pages.read").await?;
    let pages = store::list_pages(&state.db, caller.site.id).await?;
    let (page, params) = layouts::match_route(&pages, &query.route).ok_or_else(SiteError::not_found)?;
    if page.audience == "operator" && caller.operator.is_none() {
        return Err(operator_required());
    }
    let layout = store::load_page_layout(&state.db, &caller.site, page)
        .await?
        .ok_or_else(SiteError::not_found)?;
    let bindings = layouts::binding_keys(&state.db, caller.site.id).await?;
    layouts::validate_site_layout(&layout.spec, bindings).map_err(SiteError::from)?;
    Ok(ok(
        StatusCode::OK,
        json!({
            "route": page.route,
            "audience": page.audience,
            "params": params,
            "layout": layouts::with_binding_refs(&layout.spec),
        }),
    ))
}

/// `GET /bindings/{b}/fields`: the fields of the caller's audiences. Anonymous: the visitor's
/// read and submit fields only.
pub async fn fields(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
    Path((_slug, key)): Path<(String, String)>,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.fields.read").await?;
    let binding = Binding::load(&state, &caller, &key).await?;
    let mut keys: Vec<String> = Vec::new();
    let mut add = |list: Vec<String>| {
        for key in list {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    };
    if let Some(read) = binding.projection.read(Audience::Visitor) {
        add(effective_read(read, &binding.fields, &binding.form.title_template, &BTreeSet::new()).fields);
    }
    add(records::submit_fields(&binding));
    if let Some(operator) = &caller.operator {
        if let Some(access) = records::read_access(&state, &caller, &binding).await? {
            add(access.read.fields);
        }
        let excluded = records::operator_excluded(&state, &binding, operator).await?;
        add(records::update_fields(&binding, &excluded));
    }
    let fields = field_items(&keys, &binding.fields);
    Ok(ok(StatusCode::OK, json!({ "fields": fields })))
}

/// `GET /bindings/{b}/records?page=&per_page=&filter[<field>]=&sort=<field>|-<field>`
#[allow(clippy::implicit_hasher)]
pub async fn list_records(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
    Path((_slug, key)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.records.read/list").await?;
    let binding = Binding::load(&state, &caller, &key).await?;
    let request = list_request(&query)?;
    let Some(access) = records::read_access(&state, &caller, &binding).await? else {
        return Err(SiteError::not_found());
    };
    let (items, total) = records::list(&state, &caller, &binding, &access, &request).await?;
    let per_page = i64::try_from(request.per_page).unwrap_or(i64::MAX);
    let page = i64::try_from(request.page).unwrap_or(1);
    Ok(ok(
        StatusCode::OK,
        PaginatedData {
            items,
            total,
            page,
            per_page,
            total_pages: if total == 0 {
                0
            } else {
                (total + per_page - 1) / per_page
            },
        },
    ))
}

fn bad_query(message: &str) -> SiteError {
    SiteError::new(StatusCode::BAD_REQUEST, "query_invalid", message.to_string())
}

fn list_request<S: std::hash::BuildHasher>(query: &HashMap<String, String, S>) -> Result<ListRequest, SiteError> {
    let number = |name: &str, default: u64| -> Result<u64, SiteError> {
        query.get(name).map_or(Ok(default), |value| {
            value
                .parse::<u64>()
                .map_err(|_| bad_query("page and per_page are positive integers"))
        })
    };
    let page = number("page", 1)?.max(1);
    let per_page = number("per_page", 20)?.clamp(1, MAX_PER_PAGE);
    let mut filters = Vec::new();
    for (name, value) in query {
        if let Some(field) = name.strip_prefix("filter[").and_then(|rest| rest.strip_suffix(']')) {
            filters.push((field.to_string(), value.clone()));
        } else if !matches!(name.as_str(), "page" | "per_page" | "sort") {
            return Err(bad_query("unknown query parameter"));
        }
    }
    filters.sort();
    let sort = query.get("sort").filter(|value| !value.is_empty()).map(|value| {
        value
            .strip_prefix('-')
            .map_or_else(|| (value.clone(), false), |field| (field.to_string(), true))
    });
    Ok(ListRequest {
        page,
        per_page,
        filters,
        sort,
    })
}

/// `GET /bindings/{b}/records/{id}`
pub async fn get_record(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
    Path((_slug, key, id)): Path<(String, String, Uuid)>,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.records.read").await?;
    let binding = Binding::load(&state, &caller, &key).await?;
    let access = records::read_access(&state, &caller, &binding)
        .await?
        .ok_or_else(SiteError::not_found)?;
    let record = records::get(&state, &caller, &binding, &access, id).await?;
    Ok(ok(StatusCode::OK, record))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    values: Map<String, Value>,
}

/// `POST /bindings/{b}/records`: a visitor (or an operator) submits the visitor's submit fields.
pub async fn submit_record(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    Extension(address): Extension<ClientAddress>,
    headers: HeaderMap,
    Path((_slug, key)): Path<(String, String)>,
    SiteJson(body): SiteJson<SubmitRequest>,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.records.submit").await?;
    let binding = Binding::load(&state, &caller, &key).await?;
    let Some(submit) = binding.projection.visitor.submit.clone() else {
        return Err(SiteError::new(
            StatusCode::FORBIDDEN,
            "submit_not_allowed",
            "this binding takes no submissions",
        ));
    };
    let allowed = records::submit_fields(&binding);
    records::ensure_writable(&body.values, &allowed)?;
    if caller.operator.is_none()
        && let Some(visitor) = caller.visitor_id
    {
        let recent = store::visitor_submissions_last_hour(&state.db, binding.form.id, caller.site.id, visitor).await?;
        if recent >= i64::from(submit.max_per_hour) {
            return Err(SiteError::rate_limited(HOUR.as_secs()));
        }
    }
    WINDOWS
        .hit(
            Bucket::Submit,
            format!("{}:{}:{}", caller.site.id, binding.row.binding_key, address.0),
            submit.max_per_hour,
        )
        .map_err(SiteError::rate_limited)?;

    let operator_role = caller
        .operator
        .as_ref()
        .map(|operator| operator.workspace_role.clone())
        .unwrap_or_default();
    // An operator submits under their own form policy too: `record.create` is required.
    if caller.operator.is_some()
        && !crate::forms::permissions::form_action_allowed(&state, binding.form.id, &operator_role, "record.create")
            .await?
    {
        return Err(SiteError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not create these records",
        ));
    }
    // The form's plugin rules are loaded before the transaction opens.
    let rules = crate::extensions::rules::PreparedRules::prepare(
        &state,
        binding.form.workspace_id,
        binding.form.project_id,
        &binding.form.schema,
    )
    .await?;
    let tx = state.db.begin().await?;
    let mut new_cookie = None;
    let (actor, source);
    if let Some(operator) = &caller.operator {
        source = json!({
            "kind": "site",
            "site_id": caller.site.id,
            "binding": binding.row.binding_key,
            "operator_id": operator.user_id,
        });
        actor = CreateActor {
            caller: RuleCaller {
                actor_id: Some(operator.user_id),
                role: operator.workspace_role.clone(),
                is_bot: false,
            },
            policy_role: Some(operator_role.as_str()),
            hook_role: operator_role.as_str(),
            created_by: Some(operator.user_id),
            audit_kind: "site_operator",
            audit_id: operator.user_id,
        };
    } else {
        // Only the site submit may store a visitor origin (checked by the 0137 trigger).
        tx.execute_unprepared("SET LOCAL sv.site_writer = 'on'").await?;
        let visitor_id = if let Some(visitor) = caller.visitor_id {
            visitor
        } else {
            let token = session::new_token();
            let visitor = store::insert_visitor(&tx, caller.site.id, &token.hash).await?;
            new_cookie = Some(session::visitor_cookie(&token.value, &caller.site.slug));
            visitor
        };
        source = json!({
            "kind": "site",
            "site_id": caller.site.id,
            "binding": binding.row.binding_key,
            "site_visitor_id": visitor_id,
        });
        actor = CreateActor {
            caller: RuleCaller {
                actor_id: None,
                role: "site_visitor".to_string(),
                is_bot: false,
            },
            policy_role: None,
            // Legacy formula hooks check their patch against a role policy; a visitor gets the
            // least privileged workspace role's.
            hook_role: "member",
            created_by: None,
            audit_kind: "site_visitor",
            audit_id: visitor_id,
        };
    }
    let creation = rules
        .scope(create_record_as(
            &state,
            &tx,
            &binding.form,
            actor,
            NativeRecordCreateRequest {
                record_id: None,
                values: Value::Object(body.values),
                title: None,
                source,
                idempotency_key: None,
            },
        ))
        .await
        .map_err(|error| records::redact_field_errors(error.into(), &allowed))?;
    tx.commit().await?;
    let record = finish_record_creation(&state, &binding.form, &creation).await?;
    let mut response = ok(
        StatusCode::CREATED,
        json!({ "id": record.id, "values": records::pick(&record.values, &allowed) }),
    );
    if let Some(cookie) = new_cookie {
        set_cookie(&mut response, &cookie);
    }
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    values: Map<String, Value>,
    expected_updated_at: Timestamp,
}

/// `PATCH /bindings/{b}/records/{id}`: an operator updates the operator's update fields.
pub async fn update_record(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
    Path((_slug, key, id)): Path<(String, String, Uuid)>,
    SiteJson(body): SiteJson<UpdateRequest>,
) -> Result<Response, SiteError> {
    let caller = caller(&state, site, &headers, "site.records.update").await?;
    let operator = caller.require_operator()?.clone();
    let binding = Binding::load(&state, &caller, &key).await?;
    let excluded = records::operator_excluded(&state, &binding, &operator).await?;
    let allowed = records::update_fields(&binding, &excluded);
    records::ensure_writable(&body.values, &allowed)?;
    let access = records::read_access(&state, &caller, &binding)
        .await?
        .ok_or_else(SiteError::not_found)?;
    let record = records::load_for_update(&state, &caller, &binding, &access, id).await?;
    if !crate::forms::permissions::form_action_allowed(
        &state,
        binding.form.id,
        &operator.workspace_role,
        "record.update",
    )
    .await?
    {
        return Err(SiteError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not update these records",
        ));
    }
    let updated = crate::routes::form::update_record_for_site(
        &state,
        &binding.form,
        record,
        operator.user_id,
        &operator.workspace_role,
        UpdateRecordRequest {
            values: Some(Value::Object(body.values)),
            title: None,
            source: Some(json!({
                "kind": "site",
                "site_id": caller.site.id,
                "binding": binding.row.binding_key,
                "operator_id": operator.user_id,
            })),
            idempotency_key: None,
            expected_updated_at: Some(body.expected_updated_at),
            expected_schema_version: None,
        },
    )
    .await
    .map_err(|error| records::redact_field_errors(error.into(), &allowed))?;
    let mut visible: Vec<String> = access.read.fields.clone();
    for key in &allowed {
        if !visible.contains(key) {
            visible.push(key.clone());
        }
    }
    Ok(ok(
        StatusCode::OK,
        json!({
            "id": updated.id,
            "values": records::pick(&updated.values, &visible),
            "updated_at": updated.updated_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    email: String,
    password: String,
}

fn login_failed() -> SiteError {
    SiteError::new(StatusCode::UNAUTHORIZED, "login_failed", "the sign-in failed")
}

/// `POST /auth/operator/login`: the console credential check, then membership of the site's
/// workspace and a site role. Every failure is `401 login_failed`.
pub async fn login(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    Extension(address): Extension<ClientAddress>,
    headers: HeaderMap,
    SiteJson(body): SiteJson<LoginRequest>,
) -> Result<Response, SiteError> {
    Caller::anonymous(site.clone()).admit("site.sessions.create")?;
    let plain_http = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin.starts_with("http://"));
    if plain_http && !crate::config::runtime().site.cookie_insecure {
        return Err(SiteError::new(
            StatusCode::FORBIDDEN,
            "https_required",
            "operator sign-in needs HTTPS",
        ));
    }
    let email = body.email.trim().to_lowercase();
    WINDOWS
        .hit(Bucket::LoginAddress, address.0.clone(), LOGINS_PER_MINUTE)
        .map_err(SiteError::rate_limited)?;
    WINDOWS
        .hit(Bucket::LoginEmail, email.clone(), LOGINS_PER_MINUTE)
        .map_err(SiteError::rate_limited)?;
    let row = match crate::routes::auth::check_login_credentials(&state, &email, body.password).await {
        Ok(row) => row,
        Err(crate::error::ApiError::Database(error)) => return Err(error.into()),
        Err(_) => return Err(login_failed()),
    };
    let user_id = row
        .try_get::<String>("", "id")
        .ok()
        .and_then(|id| Uuid::parse_str(&id).ok())
        .ok_or_else(login_failed)?;
    let session_version = row.try_get::<i64>("", "session_version").map_err(|_| login_failed())?;
    let display_name = row.try_get::<String>("", "name").unwrap_or_default();
    if store::workspace_role(&state.db, site.workspace_id, user_id)
        .await?
        .is_none()
    {
        return Err(login_failed());
    }
    let role = store::load_role(&state.db, site.id, user_id)
        .await?
        .and_then(|row| SiteRole::parse(&row.role))
        .ok_or_else(login_failed)?;
    let token = session::new_token();
    store::insert_session(
        &state.db,
        &token.hash,
        site.id,
        user_id,
        session_version,
        session::OPERATOR_ABSOLUTE_SECS,
    )
    .await?;
    let mut response = ok(
        StatusCode::OK,
        json!({ "operator": { "user_id": user_id, "display_name": display_name, "role": role.as_str() } }),
    );
    set_cookie(&mut response, &session::operator_cookie(&token.value, &site.slug));
    Ok(response)
}

/// `POST /auth/logout`: ends the operator session of the cookie, if any.
pub async fn logout(
    State(state): State<AppState>,
    Extension(site): Extension<SiteRow>,
    headers: HeaderMap,
    SiteJson(_body): SiteJson<Value>,
) -> Result<Response, SiteError> {
    Caller::anonymous(site.clone()).admit("site.sessions.delete")?;
    if let Some(hash) = session::operator_hash(&headers) {
        store::delete_session(&state.db, &hash).await?;
    }
    let mut response = ok(StatusCode::OK, empty_object());
    set_cookie(&mut response, &session::clear_operator_cookie(&site.slug));
    Ok(response)
}
