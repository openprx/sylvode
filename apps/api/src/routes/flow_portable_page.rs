//! User-authenticated portable Page ZIP transport.

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use uuid::Uuid;

use crate::{
    error::ApiError, flow::portable_page_package, middleware::bot_auth::BotAuthContext, response::ApiResponse,
};

fn one_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, ApiError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().and_then(|value| value.to_str().ok());
    if let (Some(value), None) = (value, values.next()) {
        Ok(value)
    } else {
        Err(ApiError::invalid_update(format!("Expected one {name} header")))
    }
}

pub async fn export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = super::flow_v2::user(&claims, bot.as_ref())?;
    let bytes = portable_page_package::export(&state.db, object, actor).await?;
    let filename = format!("attachment; filename=\"{object}.sylvode-page.zip\"");
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, filename),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        bytes,
    ))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum PreviewMode {
    Export,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewQuery {
    mode: Option<PreviewMode>,
}

pub async fn preview(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(query): Query<PreviewQuery>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let actor = super::flow_v2::user(&claims, bot.as_ref())?;
    require_zip(&headers)?;
    Ok(ApiResponse::success(
        portable_page_package::preview(&state.db, object, actor, &bytes, query.mode.is_some()).await?,
    ))
}

pub async fn import(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let actor = super::flow_v2::user(&claims, bot.as_ref())?;
    require_zip(&headers)?;
    let expected = one_header(&headers, "x-package-sha256")?;
    let key = one_header(&headers, "idempotency-key")?;
    Ok(ApiResponse::success(
        portable_page_package::import(&state.db, object, actor, &bytes, expected, key).await?,
    ))
}

fn require_zip(headers: &HeaderMap) -> Result<(), ApiError> {
    if one_header(headers, "content-type")? != "application/zip" {
        return Err(ApiError::unsupported_format(
            "Portable Page packages require application/zip",
        ));
    }
    Ok(())
}

pub async fn export_graph(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    axum::Json(input): axum::Json<crate::flow::portable_graph_package::ExportInput>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = super::flow_v2::user(&claims, bot.as_ref())?;
    let bytes = crate::flow::portable_graph_package::export(&state.db, actor, object, input).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{object}.sylvode-graph.zip\""),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        bytes,
    ))
}

pub async fn preview_graph(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(query): Query<PreviewQuery>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let actor = super::flow_v2::user(&claims, bot.as_ref())?;
    require_zip(&headers)?;
    Ok(ApiResponse::success(
        crate::flow::portable_graph_package::preview(&state.db, actor, object, &bytes, query.mode.is_some()).await?,
    ))
}

pub async fn import_graph(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let actor = super::flow_v2::user(&claims, bot.as_ref())?;
    require_zip(&headers)?;
    let expected = one_header(&headers, "x-package-sha256")?;
    let key = one_header(&headers, "idempotency-key")?;
    Ok(ApiResponse::success(
        crate::flow::portable_graph_package::import(&state.db, actor, object, &bytes, expected, key).await?,
    ))
}
