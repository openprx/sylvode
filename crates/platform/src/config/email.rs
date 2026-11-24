//! `[email]` — the SMTP sender of notification e-mails, and `[server] public_url` it needs.
//!
//! The sender is off by default. When it is on, every key it needs must be present and well
//! formed, and the load fails otherwise. Validation messages name the key and never repeat the
//! value, so a password or a mailbox typed into the wrong key cannot reach a log through them.

use std::path::PathBuf;

use serde::Deserialize;

use super::raw::{Issues, is_placeholder, split_host_port};
use super::secret::Secret;

/// Hour of the day (UTC) after which the daily digest is sent unless `[email]` overrides it.
pub const DEFAULT_EMAIL_DIGEST_HOUR_UTC: u8 = 8;
/// Largest accepted `email.digest_hour_utc`.
pub const MAX_EMAIL_DIGEST_HOUR_UTC: u8 = 23;

/// How the connection to the SMTP server is protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmailTls {
    /// Plain connection upgraded with `STARTTLS`, which is required.
    Starttls,
    /// TLS from the first byte (SMTPS).
    Implicit,
    /// No TLS. Accepted only for a loopback `smtp_host`.
    None,
}

impl EmailTls {
    /// The port the protocol uses when `smtp_port` is not set.
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Starttls => 587,
            Self::Implicit => 465,
            Self::None => 25,
        }
    }
}

/// The validated `[email]` section; present only when `enabled = true`.
#[derive(Clone, Debug)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub tls: EmailTls,
    pub username: Option<String>,
    /// Never logged: a [`Secret`] renders as `[redacted]`.
    pub password: Option<Secret>,
    /// RFC 5322 mailbox, for example `Sylvode <sylvode@example.com>`.
    pub from: String,
    /// PEM file added to the trust roots of the TLS connection.
    pub ca_file: Option<PathBuf>,
    pub digest_hour_utc: u8,
    /// `[server] public_url`, the origin the links in e-mails start with (no trailing slash).
    pub public_url: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawEmail {
    enabled: Option<bool>,
    smtp_host: Option<String>,
    smtp_port: Option<u32>,
    tls: Option<String>,
    username: Option<String>,
    password: Option<String>,
    from: Option<String>,
    ca_file: Option<String>,
    digest_hour_utc: Option<u32>,
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn validate_host(raw: &str) -> Result<String, String> {
    let host = raw.trim();
    if is_placeholder(host) {
        return Err("email.smtp_host must be a concrete host name or address".to_string());
    }
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let plain = bare.parse::<std::net::IpAddr>().is_ok()
        || bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_');
    if !plain {
        return Err("email.smtp_host must be a host name or an IP address, without scheme, port or path".to_string());
    }
    Ok(bare.to_ascii_lowercase())
}

fn validate_from(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if is_placeholder(trimmed) {
        return Err("email.from must be a concrete mailbox".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("email.from must not contain control characters".to_string());
    }
    trimmed
        .parse::<lettre::message::Mailbox>()
        .map(|_| trimmed.to_string())
        .map_err(|_| "email.from must be an RFC 5322 mailbox such as \"Sylvode <sylvode@example.com>\"".to_string())
}

/// Checks `[server] public_url`: an absolute `http` or `https` origin without a path.
///
/// Returns the origin in the form links are built from: lower-case scheme and host, no trailing
/// slash.
pub(super) fn validate_public_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if is_placeholder(trimmed) {
        return Err("server.public_url must be a concrete origin, not a placeholder".to_string());
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("server.public_url must not contain whitespace".to_string());
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(
            "server.public_url must be an absolute http or https origin such as https://sylvode.example.com"
                .to_string(),
        );
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("server.public_url must use the http or https scheme".to_string());
    }
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() {
        return Err("server.public_url names no host".to_string());
    }
    if authority.contains(['/', '?', '#', '@']) {
        return Err("server.public_url must be an origin only: no path, query, fragment or credentials".to_string());
    }
    if let Some((host, port)) = split_host_port(authority) {
        if host.is_empty() {
            return Err("server.public_url names no host".to_string());
        }
        if !port.parse::<u16>().is_ok_and(|port| port != 0) {
            return Err("server.public_url has an invalid port".to_string());
        }
    }
    Ok(format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

impl RawEmail {
    /// `None` unless the section is enabled. Whatever is present is checked even when the section
    /// is off, so a mistake is found before the day the sender is switched on.
    pub(super) fn validate(self, public_url: Option<&str>, issues: &mut Issues) -> Option<EmailConfig> {
        let enabled = self.enabled.unwrap_or(false);
        let smtp_host = self.smtp_host.and_then(|host| issues.record(validate_host(&host)));
        let from = self.from.and_then(|from| issues.record(validate_from(&from)));
        let tls = self.tls.map_or_else(
            || Some(EmailTls::Starttls),
            |raw| {
                issues.record(match raw.trim().to_ascii_lowercase().as_str() {
                    "starttls" => Ok(EmailTls::Starttls),
                    "implicit" => Ok(EmailTls::Implicit),
                    "none" => Ok(EmailTls::None),
                    _ => Err("email.tls must be starttls, implicit or none".to_string()),
                })
            },
        );
        let smtp_port = self.smtp_port.and_then(|port| {
            let port = u16::try_from(port).ok().filter(|port| *port != 0);
            if port.is_none() {
                issues.push("email.smtp_port must be between 1 and 65535");
            }
            port
        });
        let digest_hour_utc = self.digest_hour_utc.map_or(DEFAULT_EMAIL_DIGEST_HOUR_UTC, |hour| {
            u8::try_from(hour)
                .ok()
                .filter(|hour| *hour <= MAX_EMAIL_DIGEST_HOUR_UTC)
                .unwrap_or_else(|| {
                    issues.push("email.digest_hour_utc must be between 0 and 23");
                    DEFAULT_EMAIL_DIGEST_HOUR_UTC
                })
        });
        let username = self.username.map(|name| name.trim().to_string());
        if username.as_deref().is_some_and(str::is_empty) {
            issues.push("email.username must not be empty; remove the key to send without authentication");
        }
        let password = self.password.map(Secret::new);
        if password.as_ref().is_some_and(Secret::is_empty) {
            issues.push("email.password must not be empty; remove the key to send without authentication");
        }
        if password.is_some() && username.is_none() {
            issues.push("email.password needs email.username");
        }
        let ca_file = self.ca_file.map(|path| path.trim().to_string());
        if ca_file.as_deref().is_some_and(str::is_empty) {
            issues.push("email.ca_file must not be empty");
        }

        if !enabled {
            return None;
        }
        let Some(smtp_host) = smtp_host else {
            issues.push("email.smtp_host is required when email.enabled is true");
            return None;
        };
        let Some(from) = from else {
            issues.push("email.from is required when email.enabled is true");
            return None;
        };
        let Some(public_url) = public_url else {
            issues.push("server.public_url is required when email.enabled is true");
            return None;
        };
        let tls = tls?;
        if tls == EmailTls::None && !is_loopback_host(&smtp_host) {
            issues.push("email.tls = \"none\" is accepted only when email.smtp_host is a loopback address");
            return None;
        }
        Some(EmailConfig {
            smtp_port: smtp_port.unwrap_or_else(|| tls.default_port()),
            smtp_host,
            tls,
            username,
            password,
            from,
            ca_file: ca_file.map(PathBuf::from),
            digest_hour_utc,
            public_url: public_url.to_string(),
        })
    }
}
