//! The routes that stay outside the catalogue: the OAuth router (contract C1 §2.4, audit MF-1).
//!
//! The list is exact. The boot check scans the non-test code of the OAuth sources for `.route(...)`
//! registrations and fails unless their paths equal this list, so a route added under `oauth/**`
//! needs a decision here (or an entry in an area table), and the guard passes only these paths.
//! Every OAuth route also has an `any(..)` branch that answers while OAuth is not configured, so
//! the list names paths, not methods.

use std::collections::BTreeSet;

use super::coverage::scan_paths;

/// The paths of the routes outside the catalogue.
pub static UNCATALOGUED: &[&str] = &[
    "/.well-known/oauth-authorization-server",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/{*resource_path}",
    "/api/v1/oauth/authorization-requests",
    "/api/v1/oauth/authorization-requests/{request_id}/decision",
    "/api/v1/oauth/token",
    "/api/v1/oauth/revoke",
    "/api/v1/oauth/events/stream",
    "/api/v1/oauth/internal/token-context",
    "/api/v1/oauth/connection",
    "/api/v1/me/connected-apps",
    "/api/v1/me/connected-apps/{grant_id}",
    "/api/v1/workspaces/{workspace_id}/connected-apps",
    "/api/v1/workspaces/{workspace_id}/connected-apps/{grant_id}",
    "/api/v1/workspaces/{workspace_id}/connected-apps/settings",
    "/api/v1/admin/oauth-clients",
    "/api/v1/admin/oauth-clients/{client_id}",
];

/// The sources of the OAuth files that register routes; `every_oauth_route_file_is_scanned` keeps
/// the list complete.
pub const OAUTH_SOURCES: &[(&str, &str)] = &[
    ("authorize.rs", include_str!("../oauth/authorize.rs")),
    ("delegated.rs", include_str!("../oauth/delegated.rs")),
    ("internal.rs", include_str!("../oauth/internal.rs")),
    ("manage.rs", include_str!("../oauth/manage.rs")),
    ("metadata.rs", include_str!("../oauth/metadata.rs")),
    ("revoke.rs", include_str!("../oauth/revoke.rs")),
    ("stream.rs", include_str!("../oauth/stream.rs")),
    ("token.rs", include_str!("../oauth/token.rs")),
];

/// Whether the matched path is one of the routes outside the catalogue.
#[must_use]
pub fn is_uncatalogued(path: &str) -> bool {
    UNCATALOGUED.contains(&path)
}

/// Findings of the OAuth router against [`UNCATALOGUED`]: a path registered but not listed, and a
/// listed path no OAuth source registers.
///
/// `catalogued` are the paths of the area tables; a path in both lists is a finding too.
#[must_use]
pub fn findings(oauth_sources: &[(&str, &str)], catalogued: &BTreeSet<&str>) -> Vec<String> {
    let registered = scan_paths(oauth_sources);
    let listed: BTreeSet<&str> = UNCATALOGUED.iter().copied().collect();
    let mut found = Vec::new();
    for path in &registered {
        if !listed.contains(path.as_str()) {
            found.push(format!("OAuth route missing from UNCATALOGUED: {path}"));
        }
    }
    for path in &listed {
        if !registered.contains(*path) {
            found.push(format!("UNCATALOGUED path no OAuth source registers: {path}"));
        }
        if catalogued.contains(path) {
            found.push(format!("path both catalogued and in UNCATALOGUED: {path}"));
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{OAUTH_SOURCES, UNCATALOGUED, findings};
    use std::collections::BTreeSet;
    use std::path::Path;

    fn route_files(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir)
            .expect("the oauth directory is readable")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                route_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("an oauth file is readable");
                let head = text.split("\nmod tests {").next().unwrap_or_default();
                if head.contains(".route(") && !path.to_string_lossy().contains("test") {
                    out.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }

    /// The OAuth router's routes equal the list.
    #[test]
    fn the_oauth_router_equals_the_uncatalogued_list() {
        let catalogued: BTreeSet<&str> = crate::catalogue::entries().map(|entry| entry.path).collect();
        let found = findings(OAUTH_SOURCES, &catalogued);
        assert!(found.is_empty(), "{found:#?}");
        assert_eq!(UNCATALOGUED.len(), UNCATALOGUED.iter().collect::<BTreeSet<_>>().len());
    }

    /// A file of `oauth/**` that registers routes is in `OAUTH_SOURCES`.
    #[test]
    fn every_oauth_route_file_is_scanned() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/oauth");
        let mut files = Vec::new();
        route_files(&dir, &mut files);
        files.sort();
        let mut listed: Vec<String> = OAUTH_SOURCES
            .iter()
            .map(|(name, _)| dir.join(name).to_string_lossy().into_owned())
            .collect();
        listed.sort();
        assert_eq!(files, listed);
    }

    /// An unlisted OAuth route and an unresolvable path constant are findings.
    #[test]
    fn a_new_oauth_route_is_a_finding() {
        let sources = [(
            "x.rs",
            "const NEW_PATH: &str = \"/api/v1/oauth/new\";\nfn r() { Router::new().route(NEW_PATH, any(h)).route(OTHER, any(h)) }",
        )];
        let found = findings(&sources, &BTreeSet::new());
        assert!(
            found
                .iter()
                .any(|l| l.contains("missing from UNCATALOGUED: /api/v1/oauth/new")),
            "{found:?}"
        );
        assert!(found.iter().any(|l| l.contains("unresolved:OTHER")), "{found:?}");
    }
}
