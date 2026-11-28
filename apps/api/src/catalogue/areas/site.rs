//! Catalogue entries of the `site` router area (`router/site.rs`), contract C3 section 6.6.
//!
//! The public surface `/api/site/v1/{slug}` is exposure `site` only: no console session, bot or
//! OAuth token reaches it. The console administration follows LEAD-DECISIONS AUD-C3-M11: reading,
//! creating sites, retitling and page writes are open to bots with the `write` level (on a published
//! site a bot's page or title write is stored as a `site_page` draft for a person to approve);
//! publishing, settings,
//! bindings, roles and deletion are for people only. Site pages carry no `project_id`, so their
//! handlers resolve the project and decide for a scoped bot themselves.

use crate::catalogue::types::Scope;
use crate::catalogue::types::dsl::{
    Admin, Delete, Destructive, Entry, Find, Get, Global, Patch, Post, Project, Put, Read, Secret, Write, bot, console,
    site,
};

const SITE_BY_ID: Scope = Project(Find::Table("sites", "id"));

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    // Public site API.
    site("site.fields.read", Get, "/api/site/v1/{slug}/bindings/{binding}/fields", Read),
    site("site.manifest.read", Get, "/api/site/v1/{slug}/manifest", Read),
    site("site.pages.read", Get, "/api/site/v1/{slug}/pages", Read),
    site("site.records.read", Get, "/api/site/v1/{slug}/bindings/{binding}/records/{id}", Read),
    site("site.records.read/list", Get, "/api/site/v1/{slug}/bindings/{binding}/records", Read),
    site("site.records.submit", Post, "/api/site/v1/{slug}/bindings/{binding}/records", Write),
    site("site.records.update", Patch, "/api/site/v1/{slug}/bindings/{binding}/records/{id}", Write),
    site("site.sessions.create", Post, "/api/site/v1/{slug}/auth/operator/login", Secret),
    site("site.sessions.delete", Post, "/api/site/v1/{slug}/auth/logout", Write),
    // Console administration.
    bot("sites.bindings.read", Get, "/api/v1/sites/{id}/bindings", SITE_BY_ID, Read),
    console("sites.bindings.create", Post, "/api/v1/sites/{id}/bindings/{key}", SITE_BY_ID, Admin),
    console("sites.bindings.delete", Delete, "/api/v1/sites/{id}/bindings/{key}", SITE_BY_ID, Destructive),
    console("sites.bindings.update", Put, "/api/v1/sites/{id}/bindings/{key}", SITE_BY_ID, Admin),
    bot("sites.pages.create", Post, "/api/v1/sites/{id}/pages", SITE_BY_ID, Write),
    bot("sites.pages.delete", Delete, "/api/v1/site-pages/{page_id}", Global, Destructive),
    bot("sites.pages.read/list", Get, "/api/v1/sites/{id}/pages", SITE_BY_ID, Read),
    bot("sites.pages.update", Patch, "/api/v1/site-pages/{page_id}", Global, Write),
    bot("sites.roles.read", Get, "/api/v1/sites/{id}/roles", SITE_BY_ID, Read),
    console("sites.roles.delete", Delete, "/api/v1/sites/{id}/roles/{user_id}", SITE_BY_ID, Admin),
    console("sites.roles.update", Put, "/api/v1/sites/{id}/roles/{user_id}", SITE_BY_ID, Admin),
    console("sites.settings.update", Put, "/api/v1/sites/{id}/settings", SITE_BY_ID, Admin),
    bot("sites.sites.create", Post, "/api/v1/projects/{project_id}/sites", Project(Find::Project("project_id")), Write),
    console("sites.sites.delete", Delete, "/api/v1/sites/{id}", SITE_BY_ID, Destructive),
    console("sites.sites.publish", Post, "/api/v1/sites/{id}/publish", SITE_BY_ID, Admin),
    console("sites.sites.publish/disable", Post, "/api/v1/sites/{id}/disable", SITE_BY_ID, Admin),
    console("sites.sites.publish/unpublish", Post, "/api/v1/sites/{id}/unpublish", SITE_BY_ID, Admin),
    bot("sites.sites.read", Get, "/api/v1/sites/{id}", SITE_BY_ID, Read),
    bot("sites.sites.read/list", Get, "/api/v1/projects/{project_id}/sites", Project(Find::Project("project_id")), Read),
    bot("sites.sites.update", Patch, "/api/v1/sites/{id}", SITE_BY_ID, Write),
];

#[cfg(test)]
mod tests {
    use super::ENTRIES;
    use crate::catalogue::bot_class_counts;
    use crate::catalogue::types::{Exposure, Scope};
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            bot_class_counts(ENTRIES),
            BTreeMap::from([("no project", 2), ("project-resolved", 8)]),
            "the classes of the site routes changed"
        );
    }

    /// Every public route is site-only and resolves its site by slug; no console route is.
    #[test]
    fn the_public_routes_are_site_only() {
        for entry in ENTRIES {
            let public = entry.path.starts_with("/api/site/v1/");
            assert_eq!(public, entry.exposure.bits() == Exposure::SITE.bits(), "{}", entry.op);
            assert_eq!(public, matches!(entry.scope, Scope::Site(_)), "{}", entry.op);
            assert!(public || entry.exposure.contains(Exposure::CONSOLE), "{}", entry.op);
            assert!(!entry.exposure.contains(Exposure::OAUTH), "{}", entry.op);
        }
    }
}
