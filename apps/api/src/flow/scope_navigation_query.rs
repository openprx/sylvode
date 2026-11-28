//! Personal navigation queries with explicit pagination and bounded, authorization-filtered totals.
use super::json_value::FlowJson;
use super::{collab::authz::PermissionLevel, scope};
use crate::error::{ApiError, ApiErrorKind};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use platform::timestamp::Timestamp;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const QUERY_SCAN_MAX: usize = 1_000;
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryParams {
    pub project_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}
#[derive(Deserialize, Serialize)]
struct Cursor {
    scope: Uuid,
    user: Uuid,
    kind: String,
    revision: i64,
    stamp: Timestamp,
    id: Uuid,
}
#[derive(FromQueryResult)]
struct Row {
    id: Uuid,
    stamp: Timestamp,
}
fn cursor(raw: &str, scope: Uuid, user: Uuid, kind: &str, revision: i64) -> Result<(Timestamp, Uuid), ApiError> {
    if raw.len() > 2048 {
        return Err(ApiError::invalid_update("invalid navigation cursor"));
    }
    let c: Cursor = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| ApiError::invalid_update("invalid navigation cursor"))?,
    )
    .map_err(|_| ApiError::invalid_update("invalid navigation cursor"))?;
    if c.scope != scope || c.user != user || c.kind != kind {
        return Err(ApiError::invalid_update("cursor does not match navigation query"));
    }
    if c.revision != revision {
        return Err(ApiError::stale_frontier(
            "navigation authority changed; restart pagination",
            None,
            None,
        ));
    }
    Ok((c.stamp, c.id))
}
fn encode(scope: Uuid, user: Uuid, kind: &str, revision: i64, row: &Row) -> Result<String, ApiError> {
    Ok(URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Cursor {
            scope,
            user,
            kind: kind.into(),
            revision,
            stamp: row.stamp,
            id: row.id,
        })
        .map_err(|_| ApiError::internal())?,
    ))
}
fn aggregates(lags: &mut [i64]) -> (i64, i64) {
    lags.sort_unstable();
    let max = lags.last().copied().unwrap_or(0);
    let rank = lags.len().saturating_mul(95).div_ceil(100);
    (max, lags.get(rank.saturating_sub(1)).copied().unwrap_or(0))
}
pub async fn read(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    kind: &str,
    query: QueryParams,
) -> Result<Value, ApiError> {
    if !matches!(kind, "trash" | "projection_lag") {
        return Err(ApiError::internal());
    }
    let owner = scope::get(db, scope_id).await?;
    if owner.kind != "personal" || owner.owner_user_id != Some(user) {
        return Err(ApiError::not_found("personal scope not found"));
    }
    if query.project_id.is_some() {
        return Err(ApiError::invalid_update("personal navigation has no project partition"));
    }
    let limit = usize::try_from(super::query::validate_limit(query.limit)?).map_err(|_| ApiError::internal())?;
    let revision = scope::revision(db, scope_id).await?;
    let after = query
        .cursor
        .as_deref()
        .map(|s| cursor(s, scope_id, user, kind, revision))
        .transpose()?;
    let sql = if kind == "trash" {
        "SELECT id,archived_at AS stamp FROM flow_objects WHERE scope_id=$1 AND lifecycle_status='archived' AND ($2::timestamptz IS NULL OR (archived_at,id)<($2,$3::uuid)) ORDER BY archived_at DESC,id DESC LIMIT 1001"
    } else {
        "SELECT id,created_at AS stamp FROM flow_objects WHERE scope_id=$1 AND lifecycle_status='active' ORDER BY created_at DESC,id DESC LIMIT 1001"
    };
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        if kind == "trash" {
            vec![scope_id.into(), after.map(|a| a.0).into(), after.map(|a| a.1).into()]
        } else {
            vec![scope_id.into()]
        },
    ))
    .all(db)
    .await?;
    if kind == "projection_lag" && rows.len() > QUERY_SCAN_MAX {
        return Err(ApiError::limit_exceeded(
            "navigation authorized scan budget exceeded",
            "scan_budget",
            Some(json!(QUERY_SCAN_MAX)),
            None,
            None,
        ));
    }
    let mut authorized = Vec::new();
    let mut proofs = Vec::new();
    let mut lags = Vec::new();
    for (examined, row) in rows.into_iter().enumerate() {
        if kind == "trash" && authorized.len() > limit {
            break;
        }
        if examined >= QUERY_SCAN_MAX {
            return Err(ApiError::limit_exceeded(
                "navigation authorized scan budget exceeded",
                "scan_budget",
                Some(json!(QUERY_SCAN_MAX)),
                None,
                None,
            ));
        }
        let proof = match scope::authorize(db, row.id, user, PermissionLevel::View).await {
            Ok(p) => p,
            Err(e) if e.kind() == ApiErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let view = scope::object_view(db, row.id, user).await?;
        let value = if kind == "trash" {
            json!({"id":view.field("id"),"parent_id":view.field("parent_id"),"object_type":view.field("object_type"),"title":view.field("title"),"archived_at":view.field("archived_at"),"appearance":view.field("appearance")})
        } else {
            let lag = view
                .field("document_seq")
                .as_i64()
                .unwrap_or(0)
                .saturating_sub(view.field("projection_seq").as_i64().unwrap_or(0));
            lags.push(lag);
            json!({"object_id":row.id,"head_seq":view.field("document_seq"),"projection_seq":view.field("projection_seq"),"lag":lag})
        };
        proofs.push(proof);
        if after.is_none_or(|a| (row.stamp, row.id) < a) {
            authorized.push((row, value));
        }
    }
    for proof in proofs {
        if !scope::proof_current(db, &proof).await? {
            return Err(super::policy::authorization_read_unstable());
        }
    }
    if scope::revision(db, scope_id).await? != revision {
        return Err(super::policy::authorization_read_unstable());
    }
    let more = authorized.len() > limit;
    authorized.truncate(limit);
    let next = if more {
        authorized
            .last()
            .map(|(r, _)| encode(scope_id, user, kind, revision, r))
            .transpose()?
    } else {
        None
    };
    let items: Vec<Value> = authorized.into_iter().map(|(_, v)| v).collect();
    let mut response = if kind == "trash" {
        json!({"items":items})
    } else {
        let (max, p95) = aggregates(&mut lags);
        json!({"items":items,"max_lag":max,"p95_lag":p95})
    };
    if let Some(next) = next {
        *response.field_mut("next_cursor")? = json!(next);
    }
    Ok(response)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_rank_is_whole_authorized_scope() {
        for count in 1..20 {
            let mut values: Vec<i64> = (0..count).collect();
            assert_eq!(aggregates(&mut values), (count - 1, count - 1));
        }
        assert_eq!(aggregates(&mut []), (0, 0));
    }
    #[test]
    fn cursor_is_bound_to_scope_actor_query_and_revision() {
        let s = Uuid::new_v4();
        let u = Uuid::new_v4();
        let r = Row {
            id: Uuid::new_v4(),
            stamp: Timestamp::now(),
        };
        let c = encode(s, u, "trash", 3, &r).unwrap();
        assert_eq!(cursor(&c, s, u, "trash", 3).unwrap(), (r.stamp, r.id));
        assert!(cursor(&c, Uuid::new_v4(), u, "trash", 3).is_err());
        assert!(cursor(&c, s, Uuid::new_v4(), "trash", 3).is_err());
        assert!(cursor(&c, s, u, "projection_lag", 3).is_err());
        assert!(cursor(&c, s, u, "trash", 4).is_err());
        assert!(cursor("bad", s, u, "trash", 3).is_err());
    }
    #[test]
    fn query_bounds_are_explicit() {
        assert!(super::super::query::validate_limit(Some(0)).is_err());
        assert!(super::super::query::validate_limit(Some(101)).is_err());
        assert_eq!(
            u64::try_from(QUERY_SCAN_MAX).unwrap(),
            super::super::collab::limits::AUTHORIZED_SCAN_ROWS_MAX
        );
        assert!(serde_json::from_value::<QueryParams>(json!({"unknown":true})).is_err());
    }
}

#[cfg(test)]
mod database_tests {
    use super::*;
    use crate::routes::context::tenant_fixture::{self, exec};
    use sea_orm::{ConnectionTrait, TransactionTrait};
    #[tokio::test]
    async fn personal_query_pages_keep_whole_scope_aggregates_and_current_actor_proofs() {
        let scratch = tenant_fixture::scratch(&format!("scope_nav_{}", Uuid::new_v4().simple()))
            .await
            .expect("explicit disposable test maintenance URL required");
        let db = &scratch.db;
        db.execute_unprepared(
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
        )
        .await
        .unwrap();
        let owner = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        for id in [owner, stranger] {
            exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Navigation unit fixture','user',true)",vec![id.into(),format!("{id}@navigation-unit.test").into()]).await;
        }
        let (home, roots) = scope::personal_home(db, owner).await.unwrap();
        let root = roots.first().unwrap().object_id;
        let tx = db.begin().await.unwrap();
        let mut pages = Vec::new();
        for index in 0..4 {
            let id = Uuid::new_v4();
            scope::insert_object_with_identity(
                &tx,
                home.id,
                owner,
                "page",
                Some(root),
                &format!("Page {index}"),
                None,
                id,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
            pages.push(id);
        }
        tx.commit().await.unwrap();
        for (index, id) in pages.iter().take(2).enumerate() {
            exec(db,"UPDATE flow_objects SET lifecycle_status='archived',archived_at=now()-($2::int * interval '1 hour') WHERE id=$1",vec![(*id).into(),i32::try_from(index).unwrap().into()]).await;
        }
        let first = read(
            db,
            home.id,
            owner,
            "trash",
            QueryParams {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(first.field("items").as_array().unwrap().len(), 1);
        assert_eq!(
            *first.pointer("/items/0/id").unwrap(),
            (*pages.first().unwrap()).to_string()
        );
        let next = first.field("next_cursor").as_str().unwrap().to_owned();
        let second = read(
            db,
            home.id,
            owner,
            "trash",
            QueryParams {
                limit: Some(1),
                cursor: Some(next.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            *second.pointer("/items/0/id").unwrap(),
            (*pages.get(1).unwrap()).to_string()
        );
        assert!(second.field("next_cursor").is_null());
        assert!(
            read(db, home.id, stranger, "trash", QueryParams::default())
                .await
                .is_err()
        );
        assert!(
            read(
                db,
                home.id,
                owner,
                "trash",
                QueryParams {
                    project_id: Some(Uuid::new_v4()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
        exec(
            db,
            "UPDATE collab_documents SET head_seq=7 WHERE object_id=$1",
            vec![(*pages.get(2).unwrap()).into()],
        )
        .await;
        let lag = read(
            db,
            home.id,
            owner,
            "projection_lag",
            QueryParams {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(*lag.field("max_lag"), 7);
        assert_eq!(*lag.field("p95_lag"), 7);
        let lag_next = lag.field("next_cursor").as_str().unwrap().to_owned();
        let lag2 = read(
            db,
            home.id,
            owner,
            "projection_lag",
            QueryParams {
                limit: Some(1),
                cursor: Some(lag_next),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(*lag2.field("max_lag"), 7);
        assert_eq!(*lag2.field("p95_lag"), 7);
        scope::advance_revision(db, home.id).await.unwrap();
        assert!(
            read(
                db,
                home.id,
                owner,
                "trash",
                QueryParams {
                    cursor: Some(next),
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
        exec(db,"INSERT INTO flow_object_grants(object_id,scope_id,principal_kind,principal_id,level,granted_by) VALUES($1,$2,'user',$3,'full_access',$4),($5,$2,'user',$3,'view',$4)",vec![(*pages.get(3).unwrap()).into(),home.id.into(),stranger.into(),owner.into(),(*pages.get(2).unwrap()).into()]).await;
        let state = platform::app::AppState {
            db: db.clone(),
            cfg: platform::config::AppConfig {
                app_name: "scope-navigation-unit".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("scope-navigation-unit-fixture-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: vec![],
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        };
        let refused = super::super::scope_move::execute(
            &state,
            *pages.get(3).unwrap(),
            stranger,
            json!({"target_object_id":(*pages.get(2).unwrap())}),
            "target-edit-required",
            None,
        )
        .await;
        assert_eq!(refused.err().unwrap().kind(), ApiErrorKind::NotFound);
        assert!(
            scope::authorize(db, *pages.get(3).unwrap(), stranger, PermissionLevel::FullAccess)
                .await
                .is_ok()
        );
        assert!(
            scope::authorize(db, *pages.get(2).unwrap(), stranger, PermissionLevel::Edit)
                .await
                .is_err()
        );
        assert_eq!(
            scope::identity(db, *pages.get(3).unwrap()).await.unwrap().parent_id,
            Some(root)
        );
        let update_count = db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM collab_updates",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(update_count, 0);
        #[cfg(not(target_os = "linux"))]
        {
            let platform_refused = super::super::scope_move::execute(
                &state,
                *pages.get(3).unwrap(),
                owner,
                json!({"target_object_id":root}),
                "native-isolation-refusal",
                None,
            )
            .await
            .err()
            .unwrap();
            assert_eq!(platform_refused.kind(), ApiErrorKind::ServerRejected);
            assert_eq!(
                scope::identity(db, *pages.get(3).unwrap()).await.unwrap().parent_id,
                Some(root)
            );
            let rows = db
                .query_one(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT count(*) AS n FROM collab_updates",
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<i64>("", "n")
                .unwrap();
            assert_eq!(rows, 0);
        }
        drop(state);
        let moved = scope::identity(db, *pages.get(3).unwrap()).await.unwrap();
        let descendant = scope::identity(db, root).await.unwrap();
        assert!(
            super::super::scope_move::validate_structure(db, &moved, &descendant)
                .await
                .is_ok()
        );
        exec(
            db,
            "UPDATE flow_objects SET parent_id=$2 WHERE id=$1",
            vec![(*pages.get(2).unwrap()).into(), (*pages.get(3).unwrap()).into()],
        )
        .await;
        let target = scope::identity(db, *pages.get(2).unwrap()).await.unwrap();
        assert!(
            super::super::scope_move::validate_structure(db, &moved, &target)
                .await
                .is_err()
        );
        let count = db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM workspaces",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(count, 0);
        scratch.drop_self().await;
    }
}
