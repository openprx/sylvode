//! The e-mail sender against a local SMTP sink and, for the outbox rules, `PostgreSQL`. Without
//! `OPENPR_TEST_DATABASE_URL` the database tests report a skip and pass. No test contacts a real
//! mail server: the sink listens on 127.0.0.1 and speaks just enough SMTP.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::excessive_nesting,
    clippy::similar_names,
    clippy::doc_markdown
)]

use std::time::Duration;

use platform::config::{EmailConfig, EmailTls, Secret};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
};
use uuid::Uuid;

use super::{ERROR_MAX_CHARS, REFUSED_TEXT, SendError, Sender, backoff, scrub, send_due};

const PASSWORD: &str = "sink-password-9f3a";
const URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

/// What the sink received for one message.
#[derive(Debug, Default)]
struct Received {
    auth: Option<String>,
    mail_from: String,
    rcpt_to: Vec<String>,
    data: String,
}

/// How the sink answers `AUTH`.
#[derive(Clone, Copy)]
enum Login {
    Accept,
    /// Rejects the login and, as a hostile or careless server might, repeats what it was given.
    RejectEchoing,
}

/// A one-connection-at-a-time SMTP server on 127.0.0.1. Messages arrive on the channel.
async fn sink(login: Login) -> (u16, mpsc::UnboundedReceiver<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read);
            let mut received = Received::default();
            if write.write_all(b"220 sink ready\r\n").await.is_err() {
                continue;
            }
            let mut line = String::new();
            loop {
                line.clear();
                if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end().to_string();
                let upper = command.to_ascii_uppercase();
                let reply: String = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                    "250-sink\r\n250 AUTH PLAIN LOGIN\r\n".to_string()
                } else if upper.starts_with("AUTH") {
                    match login {
                        Login::Accept => {
                            received.auth = Some(command.clone());
                            "235 2.7.0 accepted\r\n".to_string()
                        }
                        Login::RejectEchoing => format!("535 5.7.8 bad credentials: {command}\r\n"),
                    }
                } else if upper.starts_with("MAIL FROM") {
                    received.mail_from = command["MAIL FROM:".len()..].trim().to_string();
                    "250 ok\r\n".to_string()
                } else if upper.starts_with("RCPT TO") {
                    received.rcpt_to.push(command["RCPT TO:".len()..].trim().to_string());
                    "250 ok\r\n".to_string()
                } else if upper == "DATA" {
                    if write.write_all(b"354 go\r\n").await.is_err() {
                        break;
                    }
                    let mut data = String::new();
                    loop {
                        line.clear();
                        if lines.read_line(&mut line).await.unwrap_or(0) == 0 || line == ".\r\n" {
                            break;
                        }
                        data.push_str(&line);
                    }
                    received.data = data;
                    let _ = tx.send(std::mem::take(&mut received));
                    "250 queued\r\n".to_string()
                } else if upper == "QUIT" {
                    let _ = write.write_all(b"221 bye\r\n").await;
                    break;
                } else {
                    "250 ok\r\n".to_string()
                };
                if write.write_all(reply.as_bytes()).await.is_err() {
                    break;
                }
            }
        }
    });
    (port, rx)
}

fn config(port: u16) -> EmailConfig {
    EmailConfig {
        smtp_host: "127.0.0.1".to_string(),
        smtp_port: port,
        tls: EmailTls::None,
        username: Some("sylvode".to_string()),
        password: Some(Secret::new(PASSWORD)),
        from: "Sylvode <sylvode@example.com>".to_string(),
        ca_file: None,
        digest_hour_utc: 8,
        public_url: "https://sylvode.example.com".to_string(),
    }
}

/// A sender that may reach the loopback sink.
fn sender(port: u16) -> Sender {
    Sender::new(config(port), String::new(), true).expect("sender")
}

async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    listener.local_addr().expect("addr").port()
}

#[test]
fn the_retry_schedule_is_one_five_thirty_minutes_and_two_hours() {
    assert_eq!(backoff(1), Some(Duration::from_mins(1)));
    assert_eq!(backoff(2), Some(Duration::from_mins(5)));
    assert_eq!(backoff(3), Some(Duration::from_mins(30)));
    assert_eq!(backoff(4), Some(Duration::from_hours(2)));
    assert_eq!(backoff(5), None);
}

#[test]
fn scrub_removes_the_password_and_cuts_the_text() {
    let text = scrub("login failed for hunter2\r\nsecond line hunter2", Some("hunter2"));
    assert!(!text.contains("hunter2") && !text.contains('\n'), "{text}");
    assert_eq!(scrub(&"x".repeat(2_000), None).chars().count(), ERROR_MAX_CHARS);
    assert_eq!(scrub("kept", Some("")), "kept");
}

#[tokio::test]
async fn a_message_reaches_the_sink_with_its_envelope_subject_and_body() {
    let (port, mut inbox) = sink(Login::Accept).await;
    sender(port)
        .deliver("alice@example.org", "[Sylvode] Hello", "Body line one.\n")
        .await
        .expect("delivered");
    let received = inbox.recv().await.expect("a message");
    assert_eq!(received.mail_from, "<sylvode@example.com>");
    assert_eq!(received.rcpt_to, vec!["<alice@example.org>".to_string()]);
    assert!(received.data.contains("Subject: [Sylvode] Hello"), "{}", received.data);
    assert!(
        received.data.contains("From: Sylvode <sylvode@example.com>"),
        "{}",
        received.data
    );
    assert!(received.data.contains("To: alice@example.org"), "{}", received.data);
    assert!(received.data.contains("Body line one."), "{}", received.data);
    assert!(
        received.data.to_ascii_lowercase().contains("text/plain"),
        "{}",
        received.data
    );
    assert!(received.auth.is_some(), "the credentials were presented");
}

#[tokio::test]
async fn a_rejected_login_does_not_put_the_password_into_the_error() {
    let (port, _inbox) = sink(Login::RejectEchoing).await;
    let error = sender(port)
        .deliver("alice@example.org", "s", "b")
        .await
        .expect_err("the login is rejected");
    let text = format!("{error:?} {}", error.text());
    assert!(matches!(error, SendError::Failed(_)), "{text}");
    assert!(!text.contains(PASSWORD), "{text}");
    // The server echoed the base64 of the credentials, not the password itself; the rule is that
    // the plaintext never appears.
    assert!(!format!("{:?}", config(port)).contains(PASSWORD));
}

#[tokio::test]
async fn an_invalid_recipient_is_not_retried() {
    let (port, _inbox) = sink(Login::Accept).await;
    let error = sender(port)
        .deliver("not an address", "s", "b")
        .await
        .expect_err("invalid");
    assert_eq!(error, SendError::Recipient);
    assert!(!error.retryable());
}

#[tokio::test]
async fn the_address_rules_refuse_a_loopback_host_unless_it_is_allowed() {
    let (port, mut inbox) = sink(Login::Accept).await;
    let strict = Sender::new(config(port), String::new(), false).expect("sender");
    assert_eq!(
        strict.deliver("alice@example.org", "s", "b").await,
        Err(SendError::Refused)
    );
    assert!(inbox.try_recv().is_err(), "nothing was sent");
    let allowed = Sender::new(config(port), format!("127.0.0.1:{port}"), false).expect("sender");
    allowed
        .deliver("alice@example.org", "s", "b")
        .await
        .expect("allow listed");
}

// ---- outbox rules, against PostgreSQL ----

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        if let Ok(admin) = Database::connect(&admin_url).await {
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

async fn scratch(label: &str) -> Option<Scratch> {
    let Ok(admin_url) = std::env::var(URL_ENV) else {
        eprintln!("SKIPPED (no database): set {URL_ENV} to run this test");
        return None;
    };
    let admin = Database::connect(&admin_url)
        .await
        .expect("the test database is usable");
    let name = format!("sylvode_worker_email_{label}");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .expect("reset");
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await
        .expect("create");
    let (prefix, _) = admin_url.rsplit_once('/').expect("a database url");
    let db = Database::connect(&format!("{prefix}/{name}")).await.expect("connect");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("migrations")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    for path in files {
        let sql = std::fs::read_to_string(&path).expect("read");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
    Some(Scratch { db, name, admin_url })
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("statement failed: {err}\n{sql}"));
}

async fn user(db: &DatabaseConnection, entity_type: &str, active: bool) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
         VALUES ($1, $2, '!', 'u', 'user', $3, $4)",
        vec![
            id.into(),
            format!("u-{id}@mail.test").into(),
            active.into(),
            entity_type.to_string().into(),
        ],
    )
    .await;
    id
}

async fn queue(db: &DatabaseConnection, user_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO email_outbox (id, user_id, kind, subject, body_text) VALUES ($1, $2, 'test', 'subject', 'body')",
        vec![id.into(), user_id.into()],
    )
    .await;
    id
}

#[derive(FromQueryResult, Debug)]
struct Row {
    status: String,
    attempts: i32,
    last_error: Option<String>,
    sent: bool,
    delay_secs: i64,
}

async fn row(db: &DatabaseConnection, id: Uuid) -> Row {
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, attempts, last_error, sent_at IS NOT NULL AS sent, \
         EXTRACT(EPOCH FROM (next_attempt_at - now()))::bigint AS delay_secs FROM email_outbox WHERE id = $1",
        vec![id.into()],
    ))
    .one(db)
    .await
    .expect("query")
    .expect("row")
}

#[tokio::test]
async fn a_queued_message_is_sent_once_and_marked_sent() {
    let Some(scratch) = scratch("sent_once").await else {
        return;
    };
    let db = &scratch.db;
    let (port, mut inbox) = sink(Login::Accept).await;
    let id = queue(db, user(db, "human", true).await).await;
    let sender = sender(port);
    assert_eq!(send_due(db, &sender).await.expect("tick"), 1);
    let sent = row(db, id).await;
    assert_eq!((sent.status.as_str(), sent.attempts, sent.sent), ("sent", 1, true));
    assert!(sent.last_error.is_none());
    assert!(inbox.recv().await.is_some());
    assert_eq!(
        send_due(db, &sender).await.expect("tick"),
        0,
        "a sent row is not leased again"
    );
    assert!(inbox.try_recv().is_err());
    scratch.drop_self().await;
}

#[tokio::test]
async fn refused_smtp_host_is_failed_not_sent() {
    let Some(scratch) = scratch("refused").await else {
        return;
    };
    let db = &scratch.db;
    let (port, mut inbox) = sink(Login::Accept).await;
    let id = queue(db, user(db, "human", true).await).await;
    let strict = Sender::new(config(port), String::new(), false).expect("sender");
    assert_eq!(send_due(db, &strict).await.expect("tick"), 1);
    let failed = row(db, id).await;
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.attempts, 1, "a refusal is not retried");
    assert_eq!(failed.last_error.as_deref(), Some(REFUSED_TEXT));
    assert!(inbox.try_recv().is_err(), "nothing reached the sink");
    scratch.drop_self().await;
}

#[tokio::test]
async fn outbox_retries_and_fails_after_five() {
    let Some(scratch) = scratch("retries").await else {
        return;
    };
    let db = &scratch.db;
    let id = queue(db, user(db, "human", true).await).await;
    let sender = sender(closed_port().await);
    for (attempt, wait) in [(1, 60), (2, 300), (3, 1_800), (4, 7_200)] {
        assert_eq!(send_due(db, &sender).await.expect("tick"), 1);
        let pending = row(db, id).await;
        assert_eq!((pending.status.as_str(), pending.attempts), ("pending", attempt));
        assert!(
            (wait - 10..=wait).contains(&pending.delay_secs),
            "attempt {attempt}: next try in {} s, expected about {wait}",
            pending.delay_secs
        );
        let error = pending.last_error.expect("the failure is recorded");
        assert!(
            error.chars().count() <= ERROR_MAX_CHARS && !error.contains(PASSWORD),
            "{error}"
        );
        // Not due yet: nothing is leased.
        assert_eq!(send_due(db, &sender).await.expect("tick"), 0);
        exec(
            db,
            "UPDATE email_outbox SET next_attempt_at = now() WHERE id = $1",
            vec![id.into()],
        )
        .await;
    }
    assert_eq!(send_due(db, &sender).await.expect("tick"), 1);
    let failed = row(db, id).await;
    assert_eq!((failed.status.as_str(), failed.attempts), ("failed", 5));
    exec(
        db,
        "UPDATE email_outbox SET next_attempt_at = now() WHERE id = $1",
        vec![id.into()],
    )
    .await;
    assert_eq!(send_due(db, &sender).await.expect("tick"), 0, "a failed row is final");
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_lease_that_expired_after_the_last_attempt_is_failed_not_sent_again() {
    let Some(scratch) = scratch("stale_lease").await else {
        return;
    };
    let db = &scratch.db;
    let (port, mut inbox) = sink(Login::Accept).await;
    let stale = queue(db, user(db, "human", true).await).await;
    let retried = queue(db, user(db, "human", true).await).await;
    exec(
        db,
        "UPDATE email_outbox SET status = 'sending', attempts = 5, lease_expires_at = now() - interval '1 minute' \
         WHERE id = $1",
        vec![stale.into()],
    )
    .await;
    exec(
        db,
        "UPDATE email_outbox SET status = 'sending', attempts = 2, lease_expires_at = now() - interval '1 minute' \
         WHERE id = $1",
        vec![retried.into()],
    )
    .await;
    assert_eq!(send_due(db, &sender(port)).await.expect("tick"), 1);
    assert_eq!(row(db, stale).await.status, "failed");
    let again = row(db, retried).await;
    assert_eq!((again.status.as_str(), again.attempts), ("sent", 3));
    assert!(inbox.recv().await.is_some());
    assert!(inbox.try_recv().is_err());
    scratch.drop_self().await;
}

#[tokio::test]
async fn bots_and_deactivated_accounts_get_no_e_mail() {
    let Some(scratch) = scratch("no_bots").await else {
        return;
    };
    let db = &scratch.db;
    let (port, mut inbox) = sink(Login::Accept).await;
    let bot = queue(db, user(db, "bot", true).await).await;
    let inactive = queue(db, user(db, "human", false).await).await;
    assert_eq!(send_due(db, &sender(port)).await.expect("tick"), 2);
    for id in [bot, inactive] {
        let failed = row(db, id).await;
        assert_eq!(failed.status, "failed");
        assert!(failed.last_error.is_some());
    }
    assert!(inbox.try_recv().is_err(), "nothing was sent");
    scratch.drop_self().await;
}
