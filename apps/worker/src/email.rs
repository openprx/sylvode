//! Sends the e-mails the API queued in `email_outbox`, and queues the daily digests.
//!
//! Runs only when `[email] enabled = true`. Every 10 seconds the sender leases up to 20 due rows,
//! delivers each over SMTP and marks it `sent`, or schedules another attempt (1 minute, 5 minutes,
//! 30 minutes, 2 hours) and marks it `failed` after the fifth. Every 5 minutes the users who are
//! due a digest get one (see `api::services::notifications::queue_due_digests`).
//!
//! The SMTP host is resolved through the `[outbound]` address rules and the connection goes to the
//! addresses that passed them, with the TLS server name set to `smtp_host`. A host the rules refuse
//! fails the row at once: waiting cannot change the answer. Nothing here logs or stores the SMTP
//! password: error texts are cut and the password is removed from them before they reach a log
//! line or `last_error`.

use std::net::IpAddr;
use std::time::Duration;

use api::{
    outbound::{OutboundHostError, resolve_outbound_host},
    services::notifications::queue_due_digests,
};
use lettre::{
    Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Certificate, Tls, TlsParameters},
    },
};
use platform::{
    config::{EmailConfig, EmailTls},
    timestamp::Timestamp,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

/// Seconds between two looks at the outbox.
const SEND_INTERVAL: Duration = Duration::from_secs(10);
/// Seconds between two looks for users who are due a digest.
const DIGEST_INTERVAL: Duration = Duration::from_mins(5);
/// Rows leased at once.
const BATCH: i64 = 20;
/// Seconds a leased row stays ours before another worker may take it back.
const LEASE_SECONDS: i32 = 120;
/// Attempts after which a row is `failed`.
const MAX_ATTEMPTS: i32 = 5;
/// Longest `last_error`, in characters.
const ERROR_MAX_CHARS: usize = 500;
/// Time given to one SMTP conversation.
const SMTP_TIMEOUT: Duration = Duration::from_secs(30);
/// What a refusal by the address rules says, on the row and in the log.
const REFUSED_TEXT: &str = "refused by the outbound address policy";

/// How long to wait before the attempt that follows attempt number `attempts`; `None` once the
/// row has used its last attempt.
pub const fn backoff(attempts: i32) -> Option<Duration> {
    match attempts {
        1 => Some(Duration::from_mins(1)),
        2 => Some(Duration::from_mins(5)),
        3 => Some(Duration::from_mins(30)),
        4 => Some(Duration::from_hours(2)),
        _ => None,
    }
}

/// Why a message was not delivered.
#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// The address rules refuse the SMTP host. Not retried.
    Refused,
    /// The recipient is not a usable address. Not retried.
    Recipient,
    /// The attempt failed; another may succeed. The text is safe to store.
    Failed(String),
}

impl SendError {
    const fn retryable(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    fn text(&self) -> String {
        match self {
            Self::Refused => REFUSED_TEXT.to_string(),
            Self::Recipient => "the recipient address is not valid".to_string(),
            Self::Failed(text) => text.clone(),
        }
    }
}

/// An error text that is safe to store and log: the password removed, the length cut.
fn scrub(text: &str, password: Option<&str>) -> String {
    let mut text = text.replace(['\r', '\n'], " ");
    if let Some(password) = password.filter(|password| !password.is_empty()) {
        text = text.replace(password, "[redacted]");
    }
    text.chars().take(ERROR_MAX_CHARS).collect()
}

/// The SMTP client of one worker.
pub struct Sender {
    config: EmailConfig,
    from: Mailbox,
    tls: Option<TlsParameters>,
    allowlist: String,
    allow_private: bool,
}

impl Sender {
    /// Prepares the client: parses the sender mailbox and loads the CA file.
    ///
    /// `allowlist` (comma-separated hosts) and `allow_private` are the `[outbound]` rules the SMTP
    /// host is checked against.
    ///
    /// # Errors
    /// The mailbox or the CA file is unusable; the message names the key, never a value.
    pub fn new(config: EmailConfig, allowlist: String, allow_private: bool) -> anyhow::Result<Self> {
        let from = config
            .from
            .parse::<Mailbox>()
            .map_err(|_| anyhow::anyhow!("email.from is not a valid mailbox"))?;
        let tls = match config.tls {
            EmailTls::None => None,
            EmailTls::Starttls | EmailTls::Implicit => {
                let mut builder = TlsParameters::builder(config.smtp_host.clone());
                if let Some(path) = &config.ca_file {
                    let pem = std::fs::read(path).map_err(|_| anyhow::anyhow!("email.ca_file cannot be read"))?;
                    let certificate = Certificate::from_pem(&pem)
                        .map_err(|_| anyhow::anyhow!("email.ca_file does not hold a PEM certificate"))?;
                    builder = builder.add_root_certificate(certificate);
                }
                Some(
                    builder
                        .build_rustls()
                        .map_err(|_| anyhow::anyhow!("the TLS settings of [email] are unusable"))?,
                )
            }
        };
        Ok(Self {
            config,
            from,
            tls,
            allowlist,
            allow_private,
        })
    }

    fn password(&self) -> Option<&str> {
        self.config.password.as_ref().map(platform::config::Secret::expose)
    }

    fn transport(&self, ip: IpAddr) -> AsyncSmtpTransport<Tokio1Executor> {
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(ip.to_string())
            .port(self.config.smtp_port)
            .timeout(Some(SMTP_TIMEOUT));
        builder = match (self.config.tls, &self.tls) {
            (EmailTls::Starttls, Some(parameters)) => builder.tls(Tls::Required(parameters.clone())),
            (EmailTls::Implicit, Some(parameters)) => builder.tls(Tls::Wrapper(parameters.clone())),
            _ => builder.tls(Tls::None),
        };
        if let (Some(username), Some(password)) = (&self.config.username, self.password()) {
            builder = builder.credentials(Credentials::new(username.clone(), password.to_owned()));
        }
        builder.build()
    }

    /// Delivers one message.
    ///
    /// # Errors
    /// See [`SendError`].
    pub async fn deliver(&self, to: &str, subject: &str, body: &str) -> Result<(), SendError> {
        let recipient = to.trim().parse::<Address>().map_err(|_| SendError::Recipient)?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(Mailbox::new(None, recipient))
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_owned())
            .map_err(|_| SendError::Failed("the message could not be built".to_string()))?;
        let addresses = resolve_outbound_host(
            &self.config.smtp_host,
            self.config.smtp_port,
            &self.allowlist,
            self.allow_private,
        )
        .await
        .map_err(|error| match error {
            OutboundHostError::Refused => SendError::Refused,
            OutboundHostError::Unresolved => SendError::Failed("the SMTP host name could not be resolved".to_string()),
        })?;
        let mut last = SendError::Failed("no address to connect to".to_string());
        for address in addresses {
            match self.transport(address.ip()).send(message.clone()).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    // A server that answered has decided; another address would answer the same.
                    let answered = error.is_response() || error.is_permanent() || error.is_transient();
                    last = SendError::Failed(scrub(&error.to_string(), self.password()));
                    if answered {
                        break;
                    }
                }
            }
        }
        Err(last)
    }
}

#[derive(FromQueryResult)]
struct Leased {
    id: Uuid,
    kind: String,
    subject: String,
    body_text: String,
    attempts: i32,
    email: String,
    is_active: bool,
    entity_type: String,
}

/// Takes the rows that are due, and gives up on those that ran out of attempts while leased.
async fn lease_due(db: &DatabaseConnection) -> Result<Vec<Leased>, sea_orm::DbErr> {
    // A lease that expired after the last attempt: the worker died while sending. Do not send again.
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE email_outbox SET status = 'failed', lease_expires_at = NULL, \
         last_error = 'the last attempt did not finish' \
         WHERE status = 'sending' AND lease_expires_at < now() AND attempts >= $1",
        vec![MAX_ATTEMPTS.into()],
    ))
    .await?;
    Leased::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH leased AS ( \
           UPDATE email_outbox SET status = 'sending', attempts = attempts + 1, \
                  lease_expires_at = now() + make_interval(secs => $2::int) \
            WHERE id IN ( \
              SELECT id FROM email_outbox \
               WHERE (status = 'pending' AND next_attempt_at <= now()) \
                  OR (status = 'sending' AND lease_expires_at < now()) \
               ORDER BY next_attempt_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, user_id, kind, subject, body_text, attempts) \
         SELECT l.id, l.kind, l.subject, l.body_text, l.attempts, u.email, u.is_active, u.entity_type \
           FROM leased l JOIN users u ON u.id = l.user_id",
        vec![BATCH.into(), LEASE_SECONDS.into()],
    ))
    .all(db)
    .await
}

/// Records the result of one attempt.
async fn finish(db: &DatabaseConnection, row: &Leased, result: &Result<(), SendError>) -> Result<(), sea_orm::DbErr> {
    let statement = match result {
        Ok(()) => Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE email_outbox SET status = 'sent', sent_at = now(), lease_expires_at = NULL, last_error = NULL \
             WHERE id = $1",
            vec![row.id.into()],
        ),
        Err(error) => {
            let delay = backoff(row.attempts).filter(|_| error.retryable());
            delay.map_or_else(
                || {
                    Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "UPDATE email_outbox SET status = 'failed', lease_expires_at = NULL, last_error = $2 \
                         WHERE id = $1",
                        vec![row.id.into(), error.text().into()],
                    )
                },
                |delay| {
                    Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "UPDATE email_outbox SET status = 'pending', lease_expires_at = NULL, last_error = $2, \
                         next_attempt_at = now() + make_interval(secs => $3::int) WHERE id = $1",
                        vec![
                            row.id.into(),
                            error.text().into(),
                            i32::try_from(delay.as_secs()).unwrap_or(i32::MAX).into(),
                        ],
                    )
                },
            )
        }
    };
    db.execute(statement).await?;
    Ok(())
}

/// Leases the due rows and delivers them; returns how many were handled.
///
/// # Errors
/// A database failure. A delivery failure is recorded on its row, not returned.
pub async fn send_due(db: &DatabaseConnection, sender: &Sender) -> Result<usize, sea_orm::DbErr> {
    let rows = lease_due(db).await?;
    for row in &rows {
        let result = if row.entity_type != "human" || !row.is_active {
            // Bots never get e-mail, and a deactivated account stops getting it.
            Err(SendError::Recipient)
        } else {
            sender.deliver(&row.email, &row.subject, &row.body_text).await
        };
        match &result {
            Ok(()) => tracing::info!(outbox_id = %row.id, kind = %row.kind, "e-mail sent"),
            Err(error) => tracing::warn!(
                outbox_id = %row.id,
                kind = %row.kind,
                attempt = row.attempts,
                error = %error.text(),
                "e-mail not sent"
            ),
        }
        finish(db, row, &result).await?;
    }
    Ok(rows.len())
}

/// The worker task: sends due e-mails and queues digests until the process ends.
pub async fn run(db: DatabaseConnection, sender: Sender) {
    let mut send_tick = tokio::time::interval(SEND_INTERVAL);
    let mut digest_tick = tokio::time::interval(DIGEST_INTERVAL);
    send_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    digest_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = send_tick.tick() => {
                if let Err(error) = send_due(&db, &sender).await {
                    tracing::warn!(error = %error, "e-mail outbox sweep failed");
                }
            }
            _ = digest_tick.tick() => {
                match queue_due_digests(&db, &sender.config, Timestamp::now()).await {
                    Ok(0) => {}
                    Ok(queued) => tracing::info!(queued, "daily digests queued"),
                    Err(error) => tracing::warn!(error = %error, "daily digest sweep failed"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
