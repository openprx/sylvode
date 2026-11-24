//! Audit records of the authorization server (`oauth_audit_events`).
//!
//! One row per security-relevant step. A record can not carry a secret by construction: the event
//! is an enum, the `detail` object only takes values of [`DetailValue`] (reason codes chosen from
//! static strings, ids, counts and flags), and the only free text, the user agent, is stripped of
//! control characters and cut to 256 characters. Tokens, codes, verifiers and consent tokens
//! have no way in; a token is identified by its row id.

use std::net::IpAddr;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use serde_json::{Map, Value};
use uuid::Uuid;

/// Longest stored user agent, in characters (`oauth_audit_ua_check`).
pub const MAX_USER_AGENT_CHARS: usize = 256;

/// The events of the audit trail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditEvent {
    AuthorizeRequested,
    AuthorizeRejected,
    AuthorizeNoAccount,
    ConsentGranted,
    ConsentDenied,
    CodeIssued,
    CodeRedeemed,
    CodeReplay,
    TokenRefreshed,
    RefreshReuseDetected,
    TokenRejected,
    TokenRevoked,
    GrantRevoked,
    ClientStatusChanged,
    WorkspaceSettingChanged,
    StreamOpened,
    StreamClosed,
}

impl AuditEvent {
    pub const ALL: [Self; 17] = [
        Self::AuthorizeRequested,
        Self::AuthorizeRejected,
        Self::AuthorizeNoAccount,
        Self::ConsentGranted,
        Self::ConsentDenied,
        Self::CodeIssued,
        Self::CodeRedeemed,
        Self::CodeReplay,
        Self::TokenRefreshed,
        Self::RefreshReuseDetected,
        Self::TokenRejected,
        Self::TokenRevoked,
        Self::GrantRevoked,
        Self::ClientStatusChanged,
        Self::WorkspaceSettingChanged,
        Self::StreamOpened,
        Self::StreamClosed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizeRequested => "authorize_requested",
            Self::AuthorizeRejected => "authorize_rejected",
            Self::AuthorizeNoAccount => "authorize_no_account",
            Self::ConsentGranted => "consent_granted",
            Self::ConsentDenied => "consent_denied",
            Self::CodeIssued => "code_issued",
            Self::CodeRedeemed => "code_redeemed",
            Self::CodeReplay => "code_replay",
            Self::TokenRefreshed => "token_refreshed",
            Self::RefreshReuseDetected => "refresh_reuse_detected",
            Self::TokenRejected => "token_rejected",
            Self::TokenRevoked => "token_revoked",
            Self::GrantRevoked => "grant_revoked",
            Self::ClientStatusChanged => "client_status_changed",
            Self::WorkspaceSettingChanged => "workspace_setting_changed",
            Self::StreamOpened => "stream_opened",
            Self::StreamClosed => "stream_closed",
        }
    }
}

/// A value of the `detail` object. Free text is not representable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailValue {
    /// A reason or check name chosen from a fixed list in the code.
    Code(&'static str),
    Id(Uuid),
    Count(i64),
    Flag(bool),
}

impl DetailValue {
    fn to_json(self) -> Value {
        match self {
            Self::Code(text) => Value::String(text.to_owned()),
            Self::Id(id) => Value::String(id.to_string()),
            Self::Count(count) => Value::from(count),
            Self::Flag(flag) => Value::Bool(flag),
        }
    }
}

/// One audit row, built with the `with_*` methods and written with [`write`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    pub event: AuditEvent,
    pub client_id: Option<String>,
    pub grant_id: Option<Uuid>,
    pub token_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    detail: Map<String, Value>,
}

impl AuditRecord {
    pub fn new(event: AuditEvent) -> Self {
        Self {
            event,
            client_id: None,
            grant_id: None,
            token_id: None,
            user_id: None,
            workspace_id: None,
            actor_id: None,
            client_ip: None,
            user_agent: None,
            detail: Map::new(),
        }
    }

    #[must_use]
    pub fn with_client(mut self, client_id: &str) -> Self {
        self.client_id = Some(client_id.to_owned());
        self
    }

    #[must_use]
    pub const fn with_grant(mut self, grant_id: Uuid) -> Self {
        self.grant_id = Some(grant_id);
        self
    }

    #[must_use]
    pub const fn with_token(mut self, token_id: Uuid) -> Self {
        self.token_id = Some(token_id);
        self
    }

    #[must_use]
    pub const fn with_user(mut self, user_id: Uuid) -> Self {
        self.user_id = Some(user_id);
        self
    }

    #[must_use]
    pub const fn with_workspace(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = Some(workspace_id);
        self
    }

    #[must_use]
    pub const fn with_actor(mut self, actor_id: Uuid) -> Self {
        self.actor_id = Some(actor_id);
        self
    }

    #[must_use]
    pub const fn with_client_ip(mut self, ip: IpAddr) -> Self {
        self.client_ip = Some(ip);
        self
    }

    /// Stores the user agent without control characters, cut to [`MAX_USER_AGENT_CHARS`].
    #[must_use]
    pub fn with_user_agent(mut self, user_agent: &str) -> Self {
        self.user_agent = Some(sanitize_user_agent(user_agent)).filter(|text| !text.is_empty());
        self
    }

    #[must_use]
    pub fn with_detail(mut self, key: &'static str, value: DetailValue) -> Self {
        self.detail.insert(key.to_owned(), value.to_json());
        self
    }

    /// The `detail` object as it will be stored.
    pub const fn detail(&self) -> &Map<String, Value> {
        &self.detail
    }
}

/// Removes control characters and keeps at most [`MAX_USER_AGENT_CHARS`] characters.
pub fn sanitize_user_agent(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_USER_AGENT_CHARS)
        .collect()
}

const INSERT_SQL: &str = "INSERT INTO oauth_audit_events \
     (id, event, client_id, grant_id, token_id, user_id, workspace_id, actor_id, client_ip, user_agent, detail) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, CAST(CAST($9 AS text) AS inet), $10, $11)";

fn insert_values(record: &AuditRecord) -> Vec<sea_orm::Value> {
    vec![
        Uuid::new_v4().into(),
        record.event.as_str().into(),
        record.client_id.clone().into(),
        record.grant_id.into(),
        record.token_id.into(),
        record.user_id.into(),
        record.workspace_id.into(),
        record.actor_id.into(),
        record.client_ip.map(|ip| ip.to_string()).into(),
        record.user_agent.clone().into(),
        Value::Object(record.detail.clone()).into(),
    ]
}

/// Appends the record. A caller on an authorization path treats a failure as a failure of the
/// step; a caller that only observes (the sampled rejection record) may log and continue.
pub async fn write<C: ConnectionTrait>(conn: &C, record: &AuditRecord) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        INSERT_SQL,
        insert_values(record),
    ))
    .await?;
    Ok(())
}

/// Appends a `token_rejected` record unless one for the same token was written in the last minute.
///
/// When the token is not known the same client address is the key. Returns whether a row was
/// written. A probing client therefore costs at most one row per minute, not one per request.
pub async fn write_token_rejected_sampled<C: ConnectionTrait>(conn: &C, record: &AuditRecord) -> Result<bool, DbErr> {
    let sql = "INSERT INTO oauth_audit_events \
         (id, event, client_id, grant_id, token_id, user_id, workspace_id, actor_id, client_ip, user_agent, detail) \
         SELECT CAST($1 AS uuid), CAST($2 AS text), CAST($3 AS text), CAST($4 AS uuid), CAST($5 AS uuid), \
                CAST($6 AS uuid), CAST($7 AS uuid), CAST($8 AS uuid), CAST(CAST($9 AS text) AS inet), \
                CAST($10 AS text), CAST($11 AS jsonb) \
         WHERE NOT EXISTS ( \
           SELECT 1 FROM oauth_audit_events a \
           WHERE a.event = 'token_rejected' \
             AND a.occurred_at > now() - interval '1 minute' \
             AND ((CAST($5 AS uuid) IS NOT NULL AND a.token_id = CAST($5 AS uuid)) \
               OR (CAST($5 AS uuid) IS NULL AND a.token_id IS NULL \
                   AND a.client_ip IS NOT DISTINCT FROM CAST(CAST($9 AS text) AS inet))))";
    let result = conn
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            insert_values(record),
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn event_names_match_the_table_constraint() {
        for event in AuditEvent::ALL {
            let name = event.as_str();
            assert!((3..=64).contains(&name.len()), "{name}");
            assert!(name.starts_with(|c: char| c.is_ascii_lowercase()), "{name}");
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{name}");
        }
        let mut names: Vec<_> = AuditEvent::ALL.iter().map(|e| e.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), AuditEvent::ALL.len());
    }

    #[test]
    fn user_agent_is_stripped_and_cut() {
        let long = format!("agent\r\n{}", "x".repeat(400));
        let cleaned = sanitize_user_agent(&long);
        assert_eq!(cleaned.chars().count(), MAX_USER_AGENT_CHARS);
        assert!(cleaned.starts_with("agentxxx"));
        assert!(!cleaned.contains('\n'));
        let record = AuditRecord::new(AuditEvent::TokenRejected).with_user_agent("\u{7}\u{8}");
        assert_eq!(record.user_agent, None);
    }

    #[test]
    fn detail_holds_only_typed_values() {
        let id = Uuid::new_v4();
        let record = AuditRecord::new(AuditEvent::AuthorizeRejected)
            .with_detail("check", DetailValue::Code("redirect_uri"))
            .with_detail("project", DetailValue::Id(id))
            .with_detail("count", DetailValue::Count(3))
            .with_detail("sampled", DetailValue::Flag(true));
        let detail = Value::Object(record.detail().clone());
        assert_eq!(
            detail,
            serde_json::json!({"check": "redirect_uri", "project": id.to_string(), "count": 3, "sampled": true})
        );
    }

    #[test]
    fn a_record_carries_exactly_the_fields_it_was_given() {
        let (grant, user, ws, actor) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let ip: IpAddr = "192.0.2.7".parse().unwrap();
        let record = AuditRecord::new(AuditEvent::GrantRevoked)
            .with_client("ramflux")
            .with_grant(grant)
            .with_user(user)
            .with_workspace(ws)
            .with_actor(actor)
            .with_client_ip(ip);
        assert_eq!(record.event, AuditEvent::GrantRevoked);
        assert_eq!(record.client_id.as_deref(), Some("ramflux"));
        assert_eq!(record.grant_id, Some(grant));
        assert_eq!(record.token_id, None);
        assert_eq!(record.user_id, Some(user));
        assert_eq!(record.workspace_id, Some(ws));
        assert_eq!(record.actor_id, Some(actor));
        assert_eq!(record.client_ip, Some(ip));
        assert!(record.detail().is_empty());
    }
}
