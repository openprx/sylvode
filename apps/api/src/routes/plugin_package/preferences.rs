//! Personal presentation preferences for protocol 3 contributions. They affect only the caller,
//! never change a binding generation and are not a security boundary.
use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, FromQueryResult, TransactionTrait};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    contribution_unavailable, extension::project_member, is_v3, statement, stored_layouts, stored_manifest_v3,
    v3_layout,
};
use crate::{
    error::{ApiError, ApiErrorKind},
    plugins::{
        package::validate_key,
        package_v3::{UI_DISPLAY, plugin_error},
    },
    response::ApiResponse,
};

const PREFERENCE_LIMIT: i64 = 256;

#[derive(Debug, PartialEq, Eq)]
struct PreferenceInput {
    hidden: bool,
    collapsed: bool,
    position: Option<i16>,
}

fn preference_invalid() -> ApiError {
    plugin_error(
        ApiErrorKind::InvalidUpdate,
        "preference_invalid",
        "a preference needs hidden, collapsed and a position between 0 and 99 or null",
    )
}

/// All three fields are required; `position` may be null but not absent.
fn parse_preference(body: &[u8]) -> Result<PreferenceInput, ApiError> {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(body) else {
        return Err(preference_invalid());
    };
    if object.len() != 3 {
        return Err(preference_invalid());
    }
    let flag = |name: &str| object.get(name).and_then(Value::as_bool).ok_or_else(preference_invalid);
    let position = match object.get("position") {
        Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .filter(|position| (0..=99).contains(position))
                .and_then(|position| i16::try_from(position).ok())
                .ok_or_else(preference_invalid)?,
        ),
        None => return Err(preference_invalid()),
    };
    Ok(PreferenceInput {
        hidden: flag("hidden")?,
        collapsed: flag("collapsed")?,
        position,
    })
}

#[derive(FromQueryResult, Serialize)]
struct StoredPreference {
    key: String,
    contribution_id: String,
    hidden: bool,
    collapsed: bool,
    position: Option<i16>,
    updated_at: Timestamp,
}

pub async fn put(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, key, contribution_id)): Path<(Uuid, String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let member = project_member(&state, &claims, project_id).await?;
    let input = parse_preference(&body)?;
    if validate_key(&key).is_err() || validate_key(&contribution_id).is_err() {
        return Err(contribution_unavailable());
    }
    let tx = state.db.begin().await?;
    // Serializes one user's writes in one project so the row limit cannot be overrun.
    tx.execute(statement(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 19103))",
        vec![format!("{}:{project_id}", member.actor_id).into()],
    ))
    .await?;
    // Shares the binding lock with binding changes and configuration reset.
    let binding = tx
        .query_one(statement(
            "SELECT b.grants,p.manifest,p.library_revoked_at IS NULL AS linked
               FROM form_plugin_bindings b JOIN form_plugin_packages p ON p.id=b.package_id
              WHERE b.project_id=$1 AND b.key=$2 FOR SHARE OF b",
            vec![project_id.into(), key.clone().into()],
        ))
        .await?
        .ok_or_else(contribution_unavailable)?;
    let grants: Vec<String> = binding.try_get("", "grants")?;
    let manifest: Value = binding.try_get("", "manifest")?;
    if !binding.try_get::<bool>("", "linked")? || !is_v3(&manifest) || !grants.iter().any(|grant| grant == UI_DISPLAY) {
        return Err(contribution_unavailable());
    }
    let manifest = stored_manifest_v3(&manifest)?;
    let layouts = stored_layouts(&tx, project_id, Some(&key)).await?;
    if !v3_layout(&manifest, &layouts)
        .iter()
        .any(|placement| placement.id == contribution_id && placement.enabled)
    {
        return Err(contribution_unavailable());
    }
    let existing = tx
        .query_one(statement(
            "SELECT 1 FROM form_plugin_user_preferences
              WHERE user_id=$1 AND project_id=$2 AND key=$3 AND contribution_id=$4",
            vec![
                member.actor_id.into(),
                project_id.into(),
                key.clone().into(),
                contribution_id.clone().into(),
            ],
        ))
        .await?
        .is_some();
    if !existing {
        let count = tx
            .query_one(statement(
                "SELECT count(*) AS count FROM form_plugin_user_preferences WHERE user_id=$1 AND project_id=$2",
                vec![member.actor_id.into(), project_id.into()],
            ))
            .await?
            .ok_or(ApiError::Internal)?
            .try_get::<i64>("", "count")?;
        if count >= PREFERENCE_LIMIT {
            return Err(plugin_error(
                ApiErrorKind::LimitExceeded,
                "preference_limit",
                "personal plugin preference limit reached for this project",
            ));
        }
    }
    let stored = StoredPreference::find_by_statement(statement(
        "INSERT INTO form_plugin_user_preferences (user_id,project_id,key,contribution_id,hidden,collapsed,position)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (user_id,project_id,key,contribution_id) DO UPDATE SET hidden=EXCLUDED.hidden,
            collapsed=EXCLUDED.collapsed,position=EXCLUDED.position,updated_at=clock_timestamp()
         RETURNING key,contribution_id,hidden,collapsed,position,updated_at",
        vec![
            member.actor_id.into(),
            project_id.into(),
            key.into(),
            contribution_id.into(),
            input.hidden.into(),
            input.collapsed.into(),
            input.position.into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?;
    tx.commit().await?;
    Ok(ApiResponse::success(stored))
}

/// Removes one of the caller's own rows. Visibility is not checked so stale rows can be cleared.
pub async fn delete_one(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, key, contribution_id)): Path<(Uuid, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let member = project_member(&state, &claims, project_id).await?;
    state
        .db
        .execute(statement(
            "DELETE FROM form_plugin_user_preferences
              WHERE user_id=$1 AND project_id=$2 AND key=$3 AND contribution_id=$4",
            vec![
                member.actor_id.into(),
                project_id.into(),
                key.into(),
                contribution_id.into(),
            ],
        ))
        .await?;
    Ok(ApiResponse::success(json!({"reset": true})))
}

/// Removes every row of the caller in this project.
pub async fn delete_all(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let member = project_member(&state, &claims, project_id).await?;
    let deleted = state
        .db
        .execute(statement(
            "DELETE FROM form_plugin_user_preferences WHERE user_id=$1 AND project_id=$2",
            vec![member.actor_id.into(), project_id.into()],
        ))
        .await?
        .rows_affected();
    Ok(ApiResponse::success(json!({"reset": deleted})))
}

#[cfg(test)]
mod tests {
    use super::{PreferenceInput, parse_preference};

    #[test]
    fn preferences_require_every_field() {
        assert_eq!(
            parse_preference(br#"{"hidden":true,"collapsed":false,"position":null}"#).ok(),
            Some(PreferenceInput {
                hidden: true,
                collapsed: false,
                position: None
            })
        );
        assert_eq!(
            parse_preference(br#"{"hidden":false,"collapsed":true,"position":99}"#)
                .ok()
                .and_then(|input| input.position),
            Some(99)
        );
        for body in [
            br#"{"hidden":true,"collapsed":false}"#.as_slice(),
            br#"{"hidden":true,"position":null}"#,
            br#"{"hidden":true,"collapsed":false,"position":100}"#,
            br#"{"hidden":true,"collapsed":false,"position":-1}"#,
            br#"{"hidden":true,"collapsed":false,"position":1.5}"#,
            br#"{"hidden":"yes","collapsed":false,"position":null}"#,
            br#"{"hidden":true,"collapsed":false,"position":null,"extra":1}"#,
            br#"{"hidden":true,"collapsed":false,"other":null}"#,
            b"[]",
            b"",
        ] {
            assert!(parse_preference(body).is_err());
        }
    }
}
