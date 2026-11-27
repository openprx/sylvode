//! The invocation log: one redacted record per invocation (contract C1 §5).
//!
//! A record holds digests and sizes, never the input, the output, a guest message or a log line.
//! Digests are `hmac-sha256` under a key derived from the server's signing secret, so a digest of a
//! low-entropy input cannot be brute-forced without the server's secret.
//!
//! The record is written to `extension_invocations` (migration 0138; readable by the workspace's
//! owners and admins, deleted after 30 days) and to `tracing` (target `sv::extensions`), and returned
//! to the caller of [`super::invoke`].

use platform::app::AppState;
use ring::{hkdf, hmac};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Serialize;
use uuid::Uuid;

const INFO: &[u8] = b"sv-ext-log-digest-v1";

struct KeyLen;

impl hkdf::KeyType for KeyLen {
    fn len(&self) -> usize {
        32
    }
}

/// The digest key derived from `secret` (HKDF-SHA256, info `sv-ext-log-digest-v1`); never stored or
/// logged. `None` only if the derivation fails, which it does not for a 32-byte output.
#[must_use]
pub fn digest_key(secret: &str) -> Option<hmac::Key> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(secret.as_bytes());
    let mut key = [0_u8; 32];
    prk.expand(&[INFO], KeyLen).ok()?.fill(&mut key).ok()?;
    Some(hmac::Key::new(hmac::HMAC_SHA256, &key))
}

/// `hmac-sha256:` and the hex of the HMAC of `bytes`.
#[must_use]
pub fn digest(key: Option<&hmac::Key>, bytes: &[u8]) -> String {
    key.map_or_else(
        || format!("hmac-sha256:{}", "0".repeat(64)),
        |key| format!("hmac-sha256:{}", hex::encode(hmac::sign(key, bytes).as_ref())),
    )
}

/// One row of the invocation log.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InvocationRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    pub generation: i32,
    /// `field-rule` or `action`.
    pub world: &'static str,
    /// The action name or rule stage.
    pub entry: String,
    pub principal_kind: &'static str,
    pub principal_id: Option<Uuid>,
    pub input_digest: String,
    pub input_size: usize,
    pub output_digest: Option<String>,
    pub output_size: Option<usize>,
    /// `ok`, `guest_error`, `refused`, `limit` or `trap`.
    pub outcome: &'static str,
    /// The refusal code of a failed invocation, or the guest's own code for `guest_error`.
    pub error_code: Option<String>,
    pub duration_ms: u64,
    pub fuel_used: Option<u64>,
    pub host_calls: u32,
    pub log_lines: u32,
    /// Host calls the checker or a handler refused as `denied`, and the last such code. These are
    /// not part of the table of 0138; they show a denied call that the guest then handled.
    pub host_denials: u32,
    pub last_denial: Option<String>,
}

/// Writes the record to the log. Only the fields above; nothing the guest wrote.
pub fn emit(record: &InvocationRecord) {
    tracing::info!(
        target: "sv::extensions",
        invocation = %record.id,
        workspace = %record.workspace_id,
        installation = %record.installation_id,
        generation = record.generation,
        world = record.world,
        entry = %record.entry,
        outcome = record.outcome,
        error_code = record.error_code.as_deref().unwrap_or(""),
        input_digest = %record.input_digest,
        input_size = record.input_size,
        output_digest = record.output_digest.as_deref().unwrap_or(""),
        output_size = record.output_size.unwrap_or(0),
        duration_ms = record.duration_ms,
        fuel_used = record.fuel_used.unwrap_or(0),
        host_calls = record.host_calls,
        log_lines = record.log_lines,
        host_denials = record.host_denials,
        "extension invocation"
    );
}

/// How long invocation rows are kept.
pub const RETENTION_DAYS: i64 = 30;

/// Writes the record to `extension_invocations`. A failure is logged and does not change the result
/// of the invocation. A record without an installation (a refusal before one was known) is not stored.
pub async fn store(state: &AppState, record: &InvocationRecord) {
    if record.installation_id.is_nil() {
        return;
    }
    let size = |value: usize| i32::try_from(value).unwrap_or(i32::MAX);
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO extension_invocations \
               (id, workspace_id, installation_id, generation, world, entry, principal_kind, principal_id, \
                input_digest, input_size, output_digest, output_size, outcome, error_code, duration_ms, \
                fuel_used, host_calls, log_lines) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)",
            vec![
                record.id.into(),
                record.workspace_id.into(),
                record.installation_id.into(),
                record.generation.into(),
                record.world.to_string().into(),
                record.entry.clone().into(),
                record.principal_kind.to_string().into(),
                record.principal_id.into(),
                record.input_digest.clone().into(),
                size(record.input_size).into(),
                record.output_digest.clone().into(),
                record.output_size.map(size).into(),
                record.outcome.to_string().into(),
                record.error_code.clone().into(),
                i32::try_from(record.duration_ms).unwrap_or(i32::MAX).into(),
                record.fuel_used.and_then(|fuel| i64::try_from(fuel).ok()).into(),
                i32::try_from(record.host_calls).unwrap_or(i32::MAX).into(),
                i32::try_from(record.log_lines).unwrap_or(i32::MAX).into(),
            ],
        ))
        .await;
    if let Err(error) = result {
        tracing::warn!(target: "sv::extensions", invocation = %record.id, %error, "the invocation record was not stored");
    }
}

/// Deletes the invocation rows older than [`RETENTION_DAYS`]; returns how many.
///
/// # Errors
/// A database failure.
pub async fn purge_old(state: &AppState) -> Result<u64, sea_orm::DbErr> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM extension_invocations WHERE created_at < now() - make_interval(days => $1::int)",
            vec![RETENTION_DAYS.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// Starts the task that deletes old invocation rows at start and then every 24 hours.
pub fn spawn_retention(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            tick.tick().await;
            match purge_old(&state).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(target: "sv::extensions", deleted, "old invocation records deleted"),
                Err(error) => tracing::warn!(target: "sv::extensions", %error, "invocation records were not purged"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{digest, digest_key};

    #[test]
    fn digests_are_keyed_and_stable() {
        let key = digest_key("secret-one");
        let other = digest_key("secret-two");
        let a = digest(key.as_ref(), b"input");
        assert_eq!(a, digest(key.as_ref(), b"input"));
        assert_ne!(a, digest(other.as_ref(), b"input"));
        assert_ne!(a, digest(key.as_ref(), b"other input"));
        assert!(a.starts_with("hmac-sha256:") && a.len() == "hmac-sha256:".len() + 64);
        assert!(!a.contains("input"));
    }
}
