//! `[site]` — the public site surface (`/api/site/v1`, contract C3 section 6.3-6.4).
//!
//! Both switches are off by default. `same_origin_trial` lets a site without its own origin be
//! served under the console origin (QA and intranet only). `cookie_insecure` drops the `Secure`
//! flag of the site cookies and allows operator login over plain HTTP; like
//! `auth.allow_insecure_cookies` it is accepted only when the API binds to a loopback address
//! (checked in `AppConfig::from_config` against the resolved `server.bind_addr`).
//!
//! `assets_dir` names the built site app; the API serves it at `mount_path` (default `/site`) when
//! `same_origin_trial` is also on (the routes answer 404 otherwise). The site build's base
//! (`VITE_SV_SITE_BASE`) must equal `mount_path` plus a trailing `/`.

use std::path::PathBuf;

use serde::Deserialize;

use super::raw::Issues;

/// The validated `[site]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteConfig {
    /// Serve sites that have no `allowed_origin` under the console origin.
    pub same_origin_trial: bool,
    /// Site cookies without `Secure`; operator login over plain HTTP.
    pub cookie_insecure: bool,
    /// Directory of the built site app, served at `mount_path` (needs `same_origin_trial`).
    pub assets_dir: Option<PathBuf>,
    /// URL path the site app is served at: starts with `/`, no trailing `/`, default `/site`.
    pub mount_path: String,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            same_origin_trial: false,
            cookie_insecure: false,
            assets_dir: None,
            mount_path: DEFAULT_MOUNT_PATH.to_string(),
        }
    }
}

/// Default of `site.mount_path`.
const DEFAULT_MOUNT_PATH: &str = "/site";

/// First path segments that belong to the API; a mount path may not start with one of them.
const RESERVED_SEGMENTS: &[&str] = &["api", "uploads", "mcp", "health", "ready", ".well-known"];

fn validate_mount_path(raw: &str) -> Result<String, String> {
    let path = raw.trim();
    let fail = |reason: &str| Err(format!("site.mount_path {reason} (got {path:?})"));
    if path == "/" {
        return fail("must not be `/` alone; the console owns the root");
    }
    if !path.starts_with('/') {
        return fail("must start with `/`");
    }
    if path.ends_with('/') {
        return fail("must not end with `/`");
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
    {
        return fail("may contain only letters, digits, `-`, `_`, `.` and `/`");
    }
    if path[1..]
        .split('/')
        .any(|segment| segment.is_empty() || segment.starts_with('.') && segment != ".well-known")
    {
        return fail("must not contain empty or dot segments (`//`, `.`, `..`)");
    }
    let first = path[1..].split('/').next().unwrap_or_default();
    if RESERVED_SEGMENTS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(first))
    {
        return fail("collides with a top-level API route (/api, /uploads, /mcp, /health, /ready, /.well-known)");
    }
    Ok(path.to_string())
}

impl SiteConfig {
    /// The directory served at `mount_path`: set only when both `same_origin_trial` and `assets_dir` are.
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
    mount_path: Option<String>,
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
        let mount_path = self
            .mount_path
            .and_then(|raw| issues.record(validate_mount_path(&raw)))
            .unwrap_or_else(|| DEFAULT_MOUNT_PATH.to_string());
        SiteConfig {
            mount_path,
            same_origin_trial: self.same_origin_trial.unwrap_or(false),
            cookie_insecure: self.cookie_insecure.unwrap_or(false),
            assets_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SiteConfig, validate_mount_path};
    use std::path::PathBuf;

    #[test]
    fn mount_path_defaults_to_site_and_accepts_plain_paths() {
        assert_eq!(SiteConfig::default().mount_path, "/site");
        for ok in ["/site", "/portal", "/a/b-c_d.e", " /portal "] {
            assert!(validate_mount_path(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn mount_path_refuses_bad_shapes_and_api_routes() {
        for bad in [
            "",
            "/",
            "site",
            "/site/",
            "/a//b",
            "/a/../b",
            "/..",
            "/./a",
            "/a b",
            "/a?x",
            "/é",
            "/api",
            "/api/x",
            "/uploads",
            "/mcp",
            "/health",
            "/ready",
            "/.well-known",
            "/API",
        ] {
            assert!(validate_mount_path(bad).is_err(), "{bad:?}");
        }
    }

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
