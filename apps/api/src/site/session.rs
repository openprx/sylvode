//! Site credentials (contract C3 section 6.3): the visitor cookie `sv_site_v`, the operator session
//! cookie `sv_site_op`, and the caller of a site request with its catalogue principal.
//!
//! Both cookies carry 32 random bytes (base64url); only their SHA-256 is stored. Console cookies
//! (`access_token`, `refresh_token`) are never read here.

use axum::http::{HeaderMap, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use platform::app::AppState;
use rand::RngCore;
use sea_orm::ConnectionTrait;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::http::SiteError;
use super::store::{self, SiteRow};
use crate::catalogue::{self, Principal, Resource, SiteRole};

pub const VISITOR_COOKIE: &str = "sv_site_v";
pub const OPERATOR_COOKIE: &str = "sv_site_op";
/// 180 days.
const VISITOR_MAX_AGE_SECS: i64 = 15_552_000;
/// An operator session ends after an hour without a request.
pub const OPERATOR_IDLE_SECS: i64 = 3_600;
/// ... and eight hours after login in any case.
pub const OPERATOR_ABSOLUTE_SECS: i64 = 28_800;

/// A fresh credential: the cookie value and the hash that is stored.
pub struct Token {
    pub(crate) value: String,
    pub(crate) hash: Vec<u8>,
}

pub fn new_token() -> Token {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let value = URL_SAFE_NO_PAD.encode(bytes);
    Token {
        hash: Sha256::digest(bytes).to_vec(),
        value,
    }
}

/// The stored hash of a cookie value; `None` unless it decodes to exactly 32 bytes.
pub fn token_hash(value: &str) -> Option<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(value.trim()).ok()?;
    (bytes.len() == 32).then(|| Sha256::digest(&bytes).to_vec())
}

fn cookie_hash(headers: &HeaderMap, name: &str) -> Option<Vec<u8>> {
    crate::routes::auth::extract_cookie_token(headers, name).and_then(|value| token_hash(&value))
}

/// The `Set-Cookie` value of a site cookie, scoped to the site's API path.
pub fn cookie(name: &str, value: &str, slug: &str, max_age: i64) -> String {
    let secure = if crate::config::runtime().site.cookie_insecure {
        ""
    } else {
        "; Secure"
    };
    format!("{name}={value}; Path=/api/site/v1/{slug}; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}")
}

pub fn visitor_cookie(value: &str, slug: &str) -> String {
    cookie(VISITOR_COOKIE, value, slug, VISITOR_MAX_AGE_SECS)
}

pub fn operator_cookie(value: &str, slug: &str) -> String {
    cookie(OPERATOR_COOKIE, value, slug, OPERATOR_ABSOLUTE_SECS)
}

pub fn clear_operator_cookie(slug: &str) -> String {
    cookie(OPERATOR_COOKIE, "", slug, 0)
}

/// The operator of a live session.
#[derive(Debug, Clone)]
pub struct Operator {
    pub(crate) user_id: Uuid,
    pub(crate) display_name: String,
    pub(crate) role: SiteRole,
    /// The user's workspace role: their own form, field and record policy still applies.
    pub(crate) workspace_role: String,
}

/// Who calls a site route.
#[derive(Debug, Clone)]
pub struct Caller {
    pub(crate) site: SiteRow,
    pub(crate) visitor_id: Option<Uuid>,
    pub(crate) operator: Option<Operator>,
}

impl Caller {
    /// Resolves the caller from the site cookies. A draft site is visible to its operators only.
    pub(crate) async fn load(state: &AppState, site: SiteRow, headers: &HeaderMap) -> Result<Self, SiteError> {
        let operator = match cookie_hash(headers, OPERATOR_COOKIE) {
            Some(hash) => live_operator(&state.db, &site, &hash).await?,
            None => None,
        };
        if !site.is_published() && operator.is_none() {
            return Err(SiteError::not_found());
        }
        let visitor_id = match cookie_hash(headers, VISITOR_COOKIE) {
            Some(hash) => store::find_visitor(&state.db, site.id, &hash).await?,
            None => None,
        };
        Ok(Self {
            site,
            visitor_id,
            operator,
        })
    }

    /// A caller without credentials, for the session routes, which read no cookie but their own.
    pub(crate) const fn anonymous(site: SiteRow) -> Self {
        Self {
            site,
            visitor_id: None,
            operator: None,
        }
    }

    /// The catalogue principal of the caller.
    pub(crate) const fn principal(&self) -> Principal {
        match &self.operator {
            Some(operator) => Principal::SiteOperator {
                site_id: self.site.id,
                workspace_id: self.site.workspace_id,
                project_id: self.site.project_id,
                user_id: operator.user_id,
                role: operator.role,
            },
            None => Principal::SiteVisitor {
                site_id: self.site.id,
                workspace_id: self.site.workspace_id,
                project_id: self.site.project_id,
                visitor_id: self.visitor_id,
            },
        }
    }

    /// The route-level check of the catalogue for operation `op`: only site-exposed operations.
    pub(crate) fn admit(&self, op: &str) -> Result<(), SiteError> {
        let entry = catalogue::by_op(op).ok_or_else(SiteError::not_found)?;
        let resource = Resource {
            workspace_id: self.site.workspace_id,
            project_id: Some(self.site.project_id),
            form_id: None,
        };
        catalogue::allowed(&self.principal(), entry, &resource).map_err(|_| {
            SiteError::new(
                StatusCode::FORBIDDEN,
                "capability_not_exposed",
                "not available on the site",
            )
        })
    }

    /// The operator, or `401 operator_required`.
    pub(crate) fn require_operator(&self) -> Result<&Operator, SiteError> {
        self.operator.as_ref().ok_or_else(operator_required)
    }
}

pub fn operator_required() -> SiteError {
    SiteError::new(
        StatusCode::UNAUTHORIZED,
        "operator_required",
        "an operator session is required",
    )
}

async fn live_operator(db: &impl ConnectionTrait, site: &SiteRow, hash: &[u8]) -> Result<Option<Operator>, SiteError> {
    let Some(row) = store::live_session(db, site.id, site.workspace_id, hash, OPERATOR_IDLE_SECS).await? else {
        return Ok(None);
    };
    let Some(role) = SiteRole::parse(&row.role) else {
        return Ok(None);
    };
    Ok(Some(Operator {
        user_id: row.user_id,
        display_name: row.display_name,
        role,
        workspace_role: row.workspace_role,
    }))
}

/// The operator hash of a request, for logout.
pub fn operator_hash(headers: &HeaderMap) -> Option<Vec<u8>> {
    cookie_hash(headers, OPERATOR_COOKIE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_to_their_hash() {
        let token = new_token();
        assert_eq!(token.value.len(), 43);
        assert_eq!(token_hash(&token.value), Some(token.hash));
        assert_eq!(token_hash("short"), None);
        assert_eq!(token_hash("!!!"), None);
    }

    #[test]
    fn cookies_are_scoped_and_strict() {
        let cookie = visitor_cookie("abc", "help");
        assert!(
            cookie.starts_with("sv_site_v=abc; Path=/api/site/v1/help; HttpOnly; SameSite=Strict; Max-Age=15552000")
        );
        assert!(clear_operator_cookie("help").contains("Max-Age=0"));
    }
}
