//! `[site]` — the public site surface (`/api/site/v1`, contract C3 section 6.3-6.4).
//!
//! Both switches are off by default. `same_origin_trial` lets a site without its own origin be
//! served under the console origin (QA and intranet only). `cookie_insecure` drops the `Secure`
//! flag of the site cookies and allows operator login over plain HTTP; like
//! `auth.allow_insecure_cookies` it is accepted only when the API binds to a loopback address
//! (checked in `AppConfig::from_config` against the resolved `server.bind_addr`).
//!
//! `assets_dir` names the built site app; the API serves it at `/site` when `same_origin_trial` is
//! also on (the routes answer 404 otherwise).

use std::path::PathBuf;

use serde::Deserialize;

use super::raw::Issues;

/// The validated `[site]` section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteConfig {
    /// Serve sites that have no `allowed_origin` under the console origin.
    pub same_origin_trial: bool,
    /// Site cookies without `Secure`; operator login over plain HTTP.
    pub cookie_insecure: bool,
    /// Directory of the built site app, served at `/site` (needs `same_origin_trial`).
    pub assets_dir: Option<PathBuf>,
}

impl SiteConfig {
    /// The directory served at `/site`: set only when both `same_origin_trial` and `assets_dir` are.
    #[must_use]
    pub fn served_assets_dir(&self) -> Option<&std::path::Path> {
        self.assets_dir.as_deref().filter(|_| self.same_origin_trial)
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSite {
    same_origin_trial: Option<bool>,
    cookie_insecure: Option<bool>,
    assets_dir: Option<String>,
}

impl RawSite {
    pub(super) fn validate(self, issues: &mut Issues) -> SiteConfig {
        let assets_dir = self.assets_dir.and_then(|dir| {
            let dir = dir.trim().to_string();
            issues.record(if dir.is_empty() {
                Err("site.assets_dir must not be empty".to_string())
            } else {
                Ok(PathBuf::from(dir))
            })
        });
        SiteConfig {
            same_origin_trial: self.same_origin_trial.unwrap_or(false),
            cookie_insecure: self.cookie_insecure.unwrap_or(false),
            assets_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SiteConfig;
    use std::path::PathBuf;

    #[test]
    fn assets_are_served_only_with_the_trial_switch_and_a_directory() {
        let dir = Some(PathBuf::from("site-dist"));
        let both = SiteConfig {
            same_origin_trial: true,
            assets_dir: dir.clone(),
            ..SiteConfig::default()
        };
        assert!(both.served_assets_dir().is_some());
        let no_trial = SiteConfig {
            assets_dir: dir,
            ..SiteConfig::default()
        };
        assert!(no_trial.served_assets_dir().is_none());
        let no_dir = SiteConfig {
            same_origin_trial: true,
            ..SiteConfig::default()
        };
        assert!(no_dir.served_assets_dir().is_none());
    }
}
