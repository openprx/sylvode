//! `POST /api/v1/oauth/revoke` (RFC 7009).
//!
//! A refresh token ends its whole grant (`client_revoked`); an access token ends only itself. A
//! token that is unknown, already revoked, of another client or of any other kind answers 200 with
//! an empty body, so the endpoint reveals nothing about what exists. A missing `token` is
//! `invalid_request`.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use platform::app::AppState;
use platform::config::OAuthConfig;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use super::audit::{self, AuditEvent, AuditRecord, DetailValue};
use super::error::{OAuthError, OAuthErrorKind};
use super::grants::{Actor, GRANT_CHANGED_CHANNEL, RevokeReason, revoke_grant};
use super::rate::{self, RateBucket, RateKey};
use super::token::{
    db_error, empty_ok, form_get, load_active_client, not_found, parse_form, rate_error, read_request, refuse_query,
    require_form_content_type, user_agent,
};
use super::tokens::{TokenKind, hash_secret, is_well_formed, kind_of};

/// Path of the revocation endpoint.
pub const REVOKE_PATH: &str = "/api/v1/oauth/revoke";

#[derive(Debug, FromQueryResult)]
struct TokenRow {
    id: Uuid,
    grant_id: Uuid,
    user_id: Uuid,
    workspace_id: Uuid,
}

const FIND_TOKEN_SQL: &str = "SELECT t.id, t.grant_id, g.user_id, g.workspace_id \
     FROM oauth_tokens t JOIN oauth_grants g ON g.id = t.grant_id \
     WHERE t.token_hash = $1 AND t.kind = $2 AND t.client_id = $3";

async fn revoke_presented(
    db: &DatabaseConnection,
    client_id: &str,
    token: &str,
    base: AuditRecord,
) -> Result<(), DbErr> {
    let kind = match kind_of(token) {
        Some(kind @ (TokenKind::Access | TokenKind::Refresh)) if is_well_formed(kind, token) => kind,
        _ => return Ok(()),
    };
    let hash = hash_secret(token);
    let tx = db.begin().await?;
    let found = TokenRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        FIND_TOKEN_SQL,
        vec![
            hash.as_hex().into(),
            if kind == TokenKind::Refresh {
                "refresh"
            } else {
                "access"
            }
            .into(),
            client_id.into(),
        ],
    ))
    .one(&tx)
    .await?;
    let Some(row) = found else {
        tx.rollback().await?;
        return Ok(());
    };
    let record = base
        .with_token(row.id)
        .with_grant(row.grant_id)
        .with_user(row.user_id)
        .with_workspace(row.workspace_id);
    if kind == TokenKind::Refresh {
        revoke_grant(&tx, row.grant_id, RevokeReason::ClientRevoked, Actor::System).await?;
        audit::write(&tx, &record.with_detail("scope", DetailValue::Code("grant"))).await?;
    } else {
        let revoked = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE oauth_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
                vec![row.id.into()],
            ))
            .await?;
        if revoked.rows_affected() > 0 {
            audit::write(&tx, &record.with_detail("scope", DetailValue::Code("token"))).await?;
            // Wake the streams of the grant so one opened with this token notices.
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT pg_notify($1, $2)",
                vec![GRANT_CHANGED_CHANNEL.into(), row.grant_id.to_string().into()],
            ))
            .await?;
        }
    }
    tx.commit().await
}

async fn revoke_endpoint(State(state): State<AppState>, req: Request) -> Response {
    match handle(&state, req).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle(state: &AppState, req: Request) -> Result<Response, OAuthError> {
    let incoming = read_request(req).await?;
    let ip = rate::client_address(&incoming.headers, incoming.peer);
    rate::enforce(&state.db, RateBucket::Revoke, &RateKey::from_ip(ip))
        .await
        .map_err(rate_error)?;
    refuse_query(&incoming)?;
    require_form_content_type(&incoming.headers)?;
    let pairs = parse_form(&incoming.body).ok_or(OAuthError::new(
        OAuthErrorKind::InvalidRequest,
        "the request body is not a valid form",
    ))?;
    let token = form_get(&pairs, "token")
        .filter(|value| !value.is_empty())
        .ok_or(OAuthError::new(OAuthErrorKind::InvalidRequest, "the token is missing"))?;
    let client_id = form_get(&pairs, "client_id")
        .filter(|value| !value.is_empty())
        .ok_or(OAuthError::new(OAuthErrorKind::InvalidRequest, "the client is missing"))?;
    let client = load_active_client(&state.db, client_id)
        .await
        .map_err(|error| db_error(&error))?
        .ok_or(OAuthError::new(
            OAuthErrorKind::InvalidClient,
            "the client is not recognised",
        ))?;

    // `token_type_hint` is a hint only: the prefix of the token decides what it is.
    let mut base = AuditRecord::new(AuditEvent::TokenRevoked)
        .with_client(&client.client_id)
        .with_client_ip(ip);
    if let Some(agent) = user_agent(&incoming.headers) {
        base = base.with_user_agent(&agent);
    }
    revoke_presented(&state.db, &client.client_id, token, base)
        .await
        .map_err(|error| db_error(&error))?;
    Ok(empty_ok())
}

/// The revocation endpoint's route. Without configuration the path answers 404.
pub fn routes(config: Option<&Arc<OAuthConfig>>) -> Router<AppState> {
    match config {
        Some(_) => Router::new().route(REVOKE_PATH, post(revoke_endpoint)),
        None => Router::new().route(REVOKE_PATH, any(not_found)),
    }
}
