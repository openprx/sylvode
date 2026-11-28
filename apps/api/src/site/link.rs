//! The address a site is opened at (console "Open site").
//!
//! * A site with `settings.allowed_origin` is deployed on its own origin: the link is that origin. A
//!   separately deployed site app is built for one site (`VITE_SV_SITE_SLUG`), so the link carries no
//!   `?site=` query.
//! * Otherwise, when the API serves the built site app (`[site] same_origin_trial` and `assets_dir`),
//!   the app is shared by every site and picks the site from `?site=<slug>`: the link is
//!   `public_url` + `mount_path` + `/?site=<slug>`; without `[server] public_url` it is relative.
//! * Otherwise nothing serves the site and there is no link.

use platform::config::SiteConfig;

/// A site's address and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteLink {
    pub url: String,
    /// `separate` or `same_origin`.
    pub mode: &'static str,
}

#[must_use]
pub fn site_link(
    allowed_origin: Option<&str>,
    slug: &str,
    site: &SiteConfig,
    public_url: Option<&str>,
) -> Option<SiteLink> {
    if let Some(origin) = allowed_origin {
        return Some(SiteLink {
            url: format!("{}/", origin.trim_end_matches('/')),
            mode: "separate",
        });
    }
    site.served_assets_dir()?;
    let origin = public_url.map_or("", |url| url.trim_end_matches('/'));
    Some(SiteLink {
        url: format!("{origin}{}/?site={slug}", site.mount_path),
        mode: "same_origin",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn served(mount_path: &str) -> SiteConfig {
        SiteConfig {
            same_origin_trial: true,
            assets_dir: Some(PathBuf::from("site-dist")),
            mount_path: mount_path.to_string(),
            ..SiteConfig::default()
        }
    }

    #[test]
    fn a_site_with_its_own_origin_links_there_without_a_query() {
        let link = site_link(Some("https://help.example.com"), "help", &SiteConfig::default(), None).expect("link");
        assert_eq!(link.url, "https://help.example.com/");
        assert_eq!(link.mode, "separate");
        // The own origin wins even when the API also serves the app.
        let link = site_link(
            Some("https://help.example.com"),
            "help",
            &served("/site"),
            Some("https://sv.example.com"),
        )
        .expect("link");
        assert_eq!(link.mode, "separate");
    }

    #[test]
    fn a_served_site_links_under_the_mount_path_with_its_slug() {
        let link = site_link(None, "help", &served("/site"), Some("https://sv.example.com")).expect("link");
        assert_eq!(link.url, "https://sv.example.com/site/?site=help");
        assert_eq!(link.mode, "same_origin");
        let link = site_link(None, "help", &served("/portal/x"), Some("https://sv.example.com/")).expect("link");
        assert_eq!(link.url, "https://sv.example.com/portal/x/?site=help");
        let link = site_link(None, "help", &served("/site"), None).expect("link");
        assert_eq!(link.url, "/site/?site=help");
    }

    #[test]
    fn a_site_nothing_serves_has_no_link() {
        assert_eq!(
            site_link(None, "help", &SiteConfig::default(), Some("https://sv.example.com")),
            None
        );
        let no_trial = SiteConfig {
            assets_dir: Some(PathBuf::from("site-dist")),
            ..SiteConfig::default()
        };
        assert_eq!(site_link(None, "help", &no_trial, None), None);
    }
}
