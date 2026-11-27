//! The operation catalogue: one entry per route of the router, with its capability id, scope,
//! risk and exposure (contract C1).
//!
//! * [`types`]: the entry and its parts; [`areas`]: one table per router file.
//! * The bot route classes of `scoped_bot` are derived from the entries ([`bot_class`]).
//! * [`allowed`]: the pure route-level checker for every kind of principal.
//! * [`boot::check`]: at start-up every router route must have exactly one entry, or the process
//!   exits; the capability ids are synchronised into `capability_ids`.
//! * [`guard::guard`]: the request-time guard layer.
//! * `GET /api/v1/catalogue`: [`handler::capabilities`].

pub mod areas;
pub mod boot;
mod check;
pub mod coverage;
pub mod guard;
pub mod handler;
pub mod principal;
pub mod refusal;
pub mod types;
pub mod uncatalogued;

use std::collections::HashMap;
use std::sync::LazyLock;

pub use check::{allowed, allowed_pre, allowed_target};
pub use principal::{Grant, InstallationView, Invoker, Principal, Resource};
pub use refusal::{Refusal, RefusalCode};
pub use types::{Entry, Exposure, Find, Method, Risk, Scope, SiteFind, Stability};

use crate::middleware::scoped_bot::{Class, RunnerRoute};

/// Every entry of every area table.
pub fn entries() -> impl Iterator<Item = &'static Entry> {
    areas::TABLES.iter().flat_map(|table| table.iter())
}

/// The entries of a path, in table order; a path has at most one per method.
static BY_PATH: LazyLock<HashMap<&'static str, Vec<&'static Entry>>> = LazyLock::new(|| {
    let mut map: HashMap<&'static str, Vec<&'static Entry>> = HashMap::new();
    for entry in entries() {
        map.entry(entry.path).or_default().push(entry);
    }
    map
});

/// The entry for a method and the router's matched path template. `HEAD` is answered like `GET`.
#[must_use]
pub fn by_route(method: &str, path: &str) -> Option<&'static Entry> {
    let method = Method::parse(if method == "HEAD" { "GET" } else { method })?;
    BY_PATH.get(path)?.iter().copied().find(|entry| entry.method == method)
}

/// Whether the path template has an entry for any method.
#[must_use]
pub fn knows_path(path: &str) -> bool {
    BY_PATH.contains_key(path)
}

/// The entry of an operation key.
#[must_use]
pub fn by_op(op: &str) -> Option<&'static Entry> {
    static BY_OP: LazyLock<HashMap<&'static str, &'static Entry>> =
        LazyLock::new(|| entries().map(|entry| (entry.op, entry)).collect());
    BY_OP.get(op).copied()
}

/// `POST` routes that only read (contract C3-4, the explicit exception list): the validators take
/// their input in a body and store nothing. A bot needs the `read` permission and `read` level for
/// them, not `write`, so a read-only bot can check a schema or a layout.
const READ_POSTS: [&str; 3] = [
    "/api/v1/forms/schema/validate",
    "/api/v1/forms/{form_id}/validate",
    "/api/v1/layouts/validate",
];

/// Whether the request reads only: a safe method, or a `POST` of the exception list.
#[must_use]
pub fn is_read_only(method: &str, matched_path: Option<&str>) -> bool {
    axum::http::Method::from_bytes(method.as_bytes()).is_ok_and(|method| method.is_safe())
        || (method == "POST" && matched_path.is_some_and(|path| READ_POSTS.contains(&path)))
}

/// The bot class of an entry, and `None` for an entry a bot never reaches.
const fn bot_route(entry: &Entry) -> Option<RunnerRoute> {
    if !entry.exposure.contains(Exposure::BOT) {
        return None;
    }
    match entry.scope.bot_class() {
        Some(class) => Some(RunnerRoute {
            method: entry.method.as_str(),
            pattern: entry.path,
            class,
            comment: entry.comment,
        }),
        None => None,
    }
}

/// The bot class and the comment flag of a route; `None` when a bot cannot reach it. A scoped bot
/// is refused on `None`, so a route is closed to it until it is classified on purpose.
#[must_use]
pub fn bot_class(method: &str, path: &str) -> Option<(Class, bool)> {
    by_route(method, path)
        .and_then(bot_route)
        .map(|route| (route.class, route.comment))
}

/// Every route a bot can reach, with its class.
pub fn bot_routes() -> impl Iterator<Item = RunnerRoute> {
    entries().filter_map(|entry| bot_route(entry))
}

/// The number of bot routes per class name of one table.
#[cfg(test)]
pub(crate) fn bot_class_counts(entries: &[Entry]) -> std::collections::BTreeMap<&'static str, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for route in entries.iter().filter_map(bot_route) {
        *counts.entry(route.class.name()).or_default() += 1;
    }
    counts
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::{bot_class, bot_routes, by_op, by_route, entries};
    use crate::catalogue::boot::findings;
    use crate::catalogue::coverage::scan;
    use crate::middleware::scoped_bot::Class;

    /// A route cannot be added to a router file without an entry, and no entry names a route that
    /// is gone; the same checks the API runs at start-up (the database sync aside).
    #[test]
    fn the_catalogue_and_the_router_agree() {
        let source = crate::router_source::text();
        let facts = scan(&[("router", source.as_str())]);
        assert!(facts.len() > 450, "the route scan found only {} routes", facts.len());
        let delegated: Vec<(&str, &str)> = crate::oauth::delegated::DELEGATED_ROUTES
            .iter()
            .map(|route| (route.method, route.pattern))
            .collect();
        let found = findings(&entries().copied().collect::<Vec<_>>(), &facts, &delegated);
        assert!(found.is_empty(), "{found:#?}");
    }

    /// The bot class table derived from the entries equals the table the catalogue replaced, line
    /// for line. Changing a line of the golden file is a reviewed change.
    #[test]
    fn the_derived_bot_classes_equal_the_golden_file() {
        let mut derived: Vec<String> = bot_routes()
            .map(|route| {
                let class = format!("{:?}", route.class).replace(", ", ",");
                let (name, find) = match class.split_once('(') {
                    Some((name, rest)) => (name.to_string(), rest.strip_suffix(')').unwrap_or(rest).to_string()),
                    None => (class.clone(), "-".to_string()),
                };
                format!("{} {} {name} {find} {}", route.method, route.pattern, route.comment)
            })
            .collect();
        derived.sort();
        let golden: Vec<&str> = include_str!("golden/bot_classes.txt").lines().collect();
        assert_eq!(derived.len(), golden.len(), "the number of bot routes changed");
        for (derived, golden) in derived.iter().zip(&golden) {
            assert_eq!(derived.as_str(), *golden);
        }
    }

    /// The operations a plugin may call are the reviewed list; widening it needs an audit.
    #[test]
    fn the_plugin_callable_operations_are_pinned() {
        let mut ops: Vec<&str> = entries()
            .filter(|entry| entry.exposure.contains(crate::catalogue::Exposure::PLUGIN))
            .map(|entry| entry.op)
            .collect();
        ops.sort_unstable();
        let golden: Vec<&str> = include_str!("golden/plugin_ops.txt").lines().collect();
        assert_eq!(ops, golden);
    }

    /// Every route of the read-POST exception list is a catalogued `Read` route with a body.
    #[test]
    fn the_read_post_exceptions_are_catalogued_read_routes() {
        for path in super::READ_POSTS {
            let entry = by_route("POST", path).expect("a catalogue entry");
            assert_eq!(entry.risk, crate::catalogue::Risk::Read, "{path}");
            assert!(super::is_read_only("POST", Some(path)));
        }
        assert!(!super::is_read_only("POST", Some("/api/v1/drafts")));
        assert!(!super::is_read_only("POST", None));
        assert!(super::is_read_only("GET", None));
    }

    #[test]
    fn lookups_find_an_entry_by_route_and_by_operation() {
        let entry = by_route("PATCH", "/api/v1/form-records/{record_id}").expect("the record update route");
        assert_eq!(entry.op, "forms.records.update");
        assert_eq!(entry.capability(), "forms.records.update");
        assert_eq!(
            by_route("HEAD", "/api/v1/forms/{form_id}").map(|e| e.op),
            Some("forms.forms.read")
        );
        assert!(by_route("GET", "/api/v1/not-a-route").is_none());
        assert_eq!(
            by_op("forms.forms.read/schema_summary").map(|e| e.capability()),
            Some("forms.forms.read")
        );
        assert!(super::knows_path("/health") && !super::knows_path("/nothing"));
    }

    /// A scoped bot's class and comment flag come from the entry.
    #[test]
    fn the_bot_class_is_derived_from_the_scope() {
        assert_eq!(
            bot_class("POST", "/api/v1/issues/{issue_id}/comments"),
            Some((Class::Project(crate::catalogue::Find::Issue("issue_id")), true))
        );
        assert_eq!(bot_class("GET", "/api/v1/catalogue"), Some((Class::NoProject, false)));
        // A console-only route and a public route are closed to a bot.
        assert_eq!(bot_class("GET", "/api/v1/admin/stats"), None);
        assert_eq!(bot_class("POST", "/api/v1/auth/login"), None);
    }
}
