//! `[site]` — the public site surface (`/api/site/v1`, contract C3 section 6.3-6.4).
//!
//! Both switches are off by default. `same_origin_trial` lets a site without its own origin be
//! served under the console origin (QA and intranet only). `cookie_insecure` drops the `Secure`
//! flag of the site cookies and allows operator login over plain HTTP; like
//! `auth.allow_insecure_cookies` it is accepted only when the API binds to a loopback address
//! (checked in `AppConfig::from_config` against the resolved `server.bind_addr`).

use serde::Deserialize;

/// The validated `[site]` section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SiteConfig {
    /// Serve sites that have no `allowed_origin` under the console origin.
    pub same_origin_trial: bool,
    /// Site cookies without `Secure`; operator login over plain HTTP.
    pub cookie_insecure: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSite {
    same_origin_trial: Option<bool>,
    cookie_insecure: Option<bool>,
}

impl RawSite {
    pub(super) fn validate(self) -> SiteConfig {
        SiteConfig {
            same_origin_trial: self.same_origin_trial.unwrap_or(false),
            cookie_insecure: self.cookie_insecure.unwrap_or(false),
        }
    }
}
