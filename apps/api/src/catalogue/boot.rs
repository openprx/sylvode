//! The boot-time coverage check (contract C1 §2.4).
//!
//! Runs in `main` after the migrations and before the router is built. Every router route must
//! have exactly one catalogue entry and the other way round, the entries must agree with the
//! authentication layer of their route, and the capability ids are synchronised into the
//! database. Any finding stops the process.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait};
use sha2::{Digest, Sha256};

use super::coverage::{AuthKind, RouteFact, scan};
use super::types::{Entry, Exposure, Method, Risk, Scope};
use super::uncatalogued::{OAUTH_SOURCES, findings as oauth_findings, is_uncatalogued};

/// What the boot check saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogueReport {
    pub entries: usize,
    pub capabilities: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// The catalogue and the router disagree; one line per finding.
    #[error("the operation catalogue does not match the router:\n{}", .0.join("\n"))]
    Findings(Vec<String>),
    /// Capability ids that are gone from the code are still referenced by installation grants.
    #[error("catalogue_id_in_use: {}", .0.join(", "))]
    IdInUse(Vec<String>),
    #[error("capability id sync failed: {0}")]
    Database(#[from] DbErr),
}

/// The verbs of a capability id.
const VERBS: [&str; 31] = [
    "read",
    "create",
    "update",
    "delete",
    "restore",
    "purge",
    "install",
    "invoke",
    "request",
    "approve",
    "disable",
    "revoke",
    "rotate",
    "export",
    "test",
    "submit",
    "archive",
    "start_voting",
    "escalate",
    "run",
    "report",
    "assign",
    "commit",
    "cancel",
    "undo",
    "connect",
    "repair",
    "recalculate",
    "sign",
    "refresh",
    "import",
];

/// Areas whose ids are never dispatchable for a plugin (contract C1 §2.4, check 6).
const NO_PLUGIN_AREAS: [&str; 11] = [
    "account",
    "workspace",
    "system",
    "integrations",
    "extensions",
    "packages",
    "market",
    "drafts",
    "legacy_plugins",
    "alerts",
    "tasks",
];

const SITE_PREFIX: &str = "/api/site/v1/";

/// Runs every check against the router sources and the code's tables, then synchronises the ids.
///
/// # Errors
/// [`BootError::Findings`] with every disagreement, [`BootError::IdInUse`] for a removed id that a
/// grant still names, or the database error of the sync.
pub async fn check(router_sources: &[(&str, &str)], db: &DatabaseConnection) -> Result<CatalogueReport, BootError> {
    let entries: Vec<Entry> = super::entries().copied().collect();
    let facts = scan(router_sources);
    let delegated: Vec<(&str, &str)> = crate::oauth::delegated::DELEGATED_ROUTES
        .iter()
        .map(|route| (route.method, route.pattern))
        .collect();
    let mut problems = findings(&entries, &facts, &delegated);
    let catalogued: BTreeSet<&str> = entries.iter().map(|entry| entry.path).collect();
    problems.extend(oauth_findings(OAUTH_SOURCES, &catalogued));
    if !problems.is_empty() {
        return Err(BootError::Findings(problems));
    }
    let ids = capability_ids(&entries);
    sync_ids(db, &ids).await?;
    Ok(CatalogueReport {
        entries: entries.len(),
        capabilities: ids.len(),
    })
}

/// One capability id as stored in `capability_ids`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityId {
    pub id: &'static str,
    pub risk: Risk,
    pub plugin_exposed: bool,
    /// SHA-256 of the sorted plugin-callable op keys of the id; `None` when it has none. A grant
    /// made against another digest is not honoured until it is approved again.
    pub plugin_ops_digest: Option<String>,
}

/// The distinct ids of the entries, sorted by id.
#[must_use]
pub fn capability_ids(entries: &[Entry]) -> Vec<CapabilityId> {
    let mut ids: BTreeMap<&'static str, (CapabilityId, Vec<&'static str>)> = BTreeMap::new();
    for entry in entries {
        let plugin = entry.exposure.contains(Exposure::PLUGIN);
        let (known, plugin_ops) = ids.entry(entry.capability()).or_insert_with(|| {
            (
                CapabilityId {
                    id: entry.capability(),
                    risk: entry.risk,
                    plugin_exposed: false,
                    plugin_ops_digest: None,
                },
                Vec::new(),
            )
        });
        known.risk = known.risk.max(entry.risk);
        if plugin {
            known.plugin_exposed = true;
            plugin_ops.push(entry.op);
        }
    }
    ids.into_values()
        .map(|(mut id, mut ops)| {
            if !ops.is_empty() {
                ops.sort_unstable();
                id.plugin_ops_digest = Some(format!("sha256:{}", hex::encode(Sha256::digest(ops.join("\n")))));
            }
            id
        })
        .collect()
}

/// Every disagreement between the entries, the router and the OAuth allow-list, as one line each.
/// `delegated` is the allow-list as `(method, pattern)`.
#[must_use]
pub fn findings(entries: &[Entry], facts: &[RouteFact], delegated: &[(&str, &str)]) -> Vec<String> {
    let mut found = Vec::new();
    check_grammar_and_uniqueness(entries, &mut found);
    let auth_of = check_router_coverage(entries, facts, &mut found);
    check_authentication(entries, &auth_of, &mut found);
    check_risk_per_id(entries, &mut found);
    check_oauth(entries, delegated, &mut found);
    check_plugin_exposure(entries, &mut found);
    check_site(entries, &mut found);
    found
}

fn label(entry: &Entry) -> String {
    format!("{} {} ({})", entry.method.as_str(), entry.path, entry.op)
}

fn is_name(part: &str, max: usize) -> bool {
    let mut chars = part.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && part.len() <= max
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Check 1: grammar of every `op`; `op` unique; `(method, path)` unique.
fn check_grammar_and_uniqueness(entries: &[Entry], found: &mut Vec<String>) {
    let mut ops = BTreeSet::new();
    let mut routes = BTreeSet::new();
    for entry in entries {
        let valid = entry.id_parts().is_some_and(|(area, resource, verb)| {
            is_name(area, 32) && is_name(resource, 48) && is_name(verb, 32) && VERBS.contains(&verb)
        }) && entry.facet().is_none_or(|facet| is_name(facet, 32));
        if !valid {
            found.push(format!("invalid operation key: {}", label(entry)));
        }
        if !ops.insert(entry.op) {
            found.push(format!("operation key used twice: {}", entry.op));
        }
        if !routes.insert((entry.method, entry.path)) {
            found.push(format!("route listed twice: {} {}", entry.method.as_str(), entry.path));
        }
    }
}

/// Check 2: router routes = catalogue entries exactly. Returns the authentication kind per route.
fn check_router_coverage(
    entries: &[Entry],
    facts: &[RouteFact],
    found: &mut Vec<String>,
) -> BTreeMap<(Method, String), AuthKind> {
    let mut auth_of = BTreeMap::new();
    for fact in facts {
        auth_of.insert((fact.method, fact.path.clone()), fact.auth);
    }
    if auth_of.len() < facts.len() {
        found.push("a route registers the same method and path twice".to_string());
    }
    let catalogued: BTreeSet<(Method, &str)> = entries.iter().map(|entry| (entry.method, entry.path)).collect();
    for (method, path) in auth_of.keys() {
        if !catalogued.contains(&(*method, path.as_str())) {
            found.push(format!("route without a catalogue entry: {} {path}", method.as_str()));
        }
    }
    for entry in entries {
        if !auth_of.contains_key(&(entry.method, entry.path.to_string())) {
            found.push(format!(
                "catalogue entry for a route no router file registers: {}",
                label(entry)
            ));
        }
    }
    auth_of
}

/// Check 3: `BOT` <=> bot-or-user; scope `Public` <=> none; `InstanceAdmin` <=> admin; `jwt` routes
/// carry `CONSOLE` only. A bot route also needs a scope a bot class exists for.
fn check_authentication(entries: &[Entry], auth_of: &BTreeMap<(Method, String), AuthKind>, found: &mut Vec<String>) {
    for entry in entries {
        let Some(auth) = auth_of.get(&(entry.method, entry.path.to_string())) else {
            continue;
        };
        let bot = entry.exposure.contains(Exposure::BOT);
        let site = entry.exposure.contains(Exposure::SITE);
        let problem = if (*auth == AuthKind::BotOrUser) != bot {
            Some("BOT exposure and the bot-or-user layer disagree")
        } else if bot && entry.scope.bot_class().is_none() {
            Some("a bot route needs a scope with a bot class")
        } else if (*auth == AuthKind::None) != (entry.scope == Scope::Public) && !site {
            Some("scope public and the missing authentication layer disagree")
        } else if (*auth == AuthKind::Admin) != (entry.scope == Scope::InstanceAdmin) {
            Some("scope instance_admin and the admin layer disagree")
        } else if *auth == AuthKind::Jwt && entry.exposure.bits() != Exposure::CONSOLE.bits() {
            Some("a jwt route carries the console exposure only")
        } else if !site && !entry.exposure.contains(Exposure::CONSOLE) {
            Some("every route except a site route carries the console exposure")
        } else {
            None
        };
        if let Some(problem) = problem {
            found.push(format!("{problem}: {}", label(entry)));
        }
    }
}

/// Check 4: all entries of one id share the risk.
fn check_risk_per_id(entries: &[Entry], found: &mut Vec<String>) {
    let mut risk_of: BTreeMap<&str, Risk> = BTreeMap::new();
    let mut reported = BTreeSet::new();
    for entry in entries {
        let known = *risk_of.entry(entry.capability()).or_insert(entry.risk);
        if known != entry.risk && reported.insert(entry.capability()) {
            found.push(format!("the routes of {} do not share one risk", entry.capability()));
        }
    }
}

/// Check 5: entries with `OAUTH` = `DELEGATED_ROUTES` as a set, where the delegated routes of the
/// OAuth router (listed in `UNCATALOGUED`) are the only ones left out.
fn check_oauth(entries: &[Entry], delegated: &[(&str, &str)], found: &mut Vec<String>) {
    let exposed: BTreeSet<(&str, &str)> = entries
        .iter()
        .filter(|entry| entry.exposure.contains(Exposure::OAUTH))
        .map(|entry| (entry.method.as_str(), entry.path))
        .collect();
    let allowed: BTreeSet<(&str, &str)> = delegated
        .iter()
        .copied()
        .filter(|(_, pattern)| !is_uncatalogued(pattern))
        .collect();
    for (method, path) in exposed.difference(&allowed) {
        found.push(format!(
            "OAUTH exposure on a route the delegated allow-list lacks: {method} {path}"
        ));
    }
    for (method, path) in allowed.difference(&exposed) {
        found.push(format!("delegated route without OAUTH exposure: {method} {path}"));
    }
}

/// Check 6: `PLUGIN` only with risk read or write, scope project and an area that may be exposed.
fn check_plugin_exposure(entries: &[Entry], found: &mut Vec<String>) {
    for entry in entries.iter().filter(|entry| entry.exposure.contains(Exposure::PLUGIN)) {
        let area = entry.id_parts().map_or("", |(area, _, _)| area);
        let area_closed = NO_PLUGIN_AREAS.contains(&area);
        let project = matches!(entry.scope, Scope::Project(_));
        if !matches!(entry.risk, Risk::Read | Risk::Write) || !project || area_closed {
            found.push(format!("PLUGIN exposure not allowed here: {}", label(entry)));
        }
    }
}

/// Check 7: `SITE` <=> a path under `/api/site/v1/`, and then no other bit.
fn check_site(entries: &[Entry], found: &mut Vec<String>) {
    for entry in entries {
        let site = entry.exposure.contains(Exposure::SITE);
        if site != entry.path.starts_with(SITE_PREFIX) || (site && entry.exposure.bits() != Exposure::SITE.bits()) {
            found.push(format!("SITE exposure and the site path disagree: {}", label(entry)));
        }
    }
}

/// The advisory lock key of the id sync ("SVCATALG").
const SYNC_LOCK_KEY: i64 = 0x5356_4341_5441_4C47;

const UPSERT_IDS: &str = "INSERT INTO capability_ids (id, risk, plugin_exposed, plugin_ops_digest) \
     SELECT id, risk, plugin_exposed, NULLIF(digest, '') \
       FROM UNNEST($1::text[], $2::text[], $3::bool[], $4::text[]) AS t(id, risk, plugin_exposed, digest) \
     ON CONFLICT (id) DO UPDATE SET risk = EXCLUDED.risk, plugin_exposed = EXCLUDED.plugin_exposed, \
        plugin_ops_digest = EXCLUDED.plugin_ops_digest, updated_at = now() \
     WHERE capability_ids.risk IS DISTINCT FROM EXCLUDED.risk \
        OR capability_ids.plugin_exposed IS DISTINCT FROM EXCLUDED.plugin_exposed \
        OR capability_ids.plugin_ops_digest IS DISTINCT FROM EXCLUDED.plugin_ops_digest";

/// Check 8, in one transaction: upsert every code id; delete database ids absent from code that no
/// grant references; a database id absent from code but referenced by `installation_grants` fails
/// the boot. The grant table arrives with a later migration, so the reference check runs only
/// when it exists.
async fn sync_ids(db: &DatabaseConnection, ids: &[CapabilityId]) -> Result<(), BootError> {
    let names: Vec<String> = ids.iter().map(|id| id.id.to_string()).collect();
    let risks: Vec<String> = ids.iter().map(|id| id.risk.as_str().to_string()).collect();
    let plugin: Vec<bool> = ids.iter().map(|id| id.plugin_exposed).collect();
    let digests: Vec<String> = ids
        .iter()
        .map(|id| id.plugin_ops_digest.clone().unwrap_or_default())
        .collect();
    let txn = db.begin().await?;
    // One sync at a time across replicas and against a concurrent approval.
    txn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        vec![SYNC_LOCK_KEY.into()],
    ))
    .await?;
    txn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        UPSERT_IDS,
        vec![names.clone().into(), risks.into(), plugin.into(), digests.into()],
    ))
    .await?;
    let stale: Vec<String> = txn
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM capability_ids WHERE NOT (id = ANY($1)) ORDER BY id",
            vec![names.into()],
        ))
        .await?
        .iter()
        .filter_map(|row| row.try_get::<String>("", "id").ok())
        .collect();
    if !stale.is_empty() {
        let grants_table = txn
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT to_regclass('public.installation_grants') IS NOT NULL AS present",
            ))
            .await?
            .and_then(|row| row.try_get::<bool>("", "present").ok())
            .unwrap_or(false);
        if grants_table {
            let in_use: Vec<String> = txn
                .query_all(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT DISTINCT capability FROM installation_grants WHERE capability = ANY($1) ORDER BY capability",
                    vec![stale.clone().into()],
                ))
                .await?
                .iter()
                .filter_map(|row| row.try_get::<String>("", "capability").ok())
                .collect();
            if !in_use.is_empty() {
                return Err(BootError::IdInUse(in_use));
            }
        }
        txn.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM capability_ids WHERE id = ANY($1)",
            vec![stale.into()],
        ))
        .await?;
    }
    txn.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{capability_ids, findings};
    use crate::catalogue::coverage::{AuthKind, RouteFact};
    use crate::catalogue::types::Method;
    use crate::catalogue::types::dsl::*;

    fn fact(method: Method, path: &str, auth: AuthKind) -> RouteFact {
        RouteFact {
            method,
            path: path.to_string(),
            auth,
        }
    }

    /// A consistent catalogue has no finding.
    #[test]
    fn a_consistent_catalogue_is_clean() {
        let entries = [
            bot(
                "forms.records.read",
                Get,
                "/api/v1/r/{id}",
                Project(Find::Project("id")),
                Read,
            )
            .oauth()
            .plugin(),
            bot(
                "forms.records.update",
                Patch,
                "/api/v1/r/{id}",
                Project(Find::Project("id")),
                Write,
            ),
            console("account.sessions.create", Post, "/api/v1/login", Public, Secret),
            console("system.stats.read", Get, "/api/v1/stats", InstanceAdmin, Admin),
            console("legacy_plugins.packages.read/mine", Get, "/api/v1/me/p", Own, Read),
        ];
        let facts = [
            fact(Method::Get, "/api/v1/r/{id}", AuthKind::BotOrUser),
            fact(Method::Patch, "/api/v1/r/{id}", AuthKind::BotOrUser),
            fact(Method::Post, "/api/v1/login", AuthKind::None),
            fact(Method::Get, "/api/v1/stats", AuthKind::Admin),
            fact(Method::Get, "/api/v1/me/p", AuthKind::Jwt),
        ];
        let delegated = [("GET", "/api/v1/r/{id}"), ("GET", "/api/v1/oauth/connection")];
        assert_eq!(findings(&entries, &facts, &delegated), Vec::<String>::new());
    }

    /// An unclassified route and a stale entry are both reported; this is how the boot check is demonstrated.
    #[test]
    fn an_unclassified_route_and_a_stale_entry_are_findings() {
        let entries = [
            bot(
                "forms.records.read",
                Get,
                "/api/v1/r/{id}",
                Project(Find::Project("id")),
                Read,
            ),
            bot(
                "forms.records.delete",
                Delete,
                "/api/v1/gone",
                Project(Find::Project("id")),
                Destructive,
            ),
        ];
        let facts = [
            fact(Method::Get, "/api/v1/r/{id}", AuthKind::BotOrUser),
            fact(Method::Post, "/api/v1/new", AuthKind::BotOrUser),
        ];
        let got = findings(&entries, &facts, &[]);
        assert!(
            got.iter()
                .any(|l| l.contains("route without a catalogue entry: POST /api/v1/new")),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|l| l.contains("no router file registers") && l.contains("/api/v1/gone")),
            "{got:?}"
        );
    }

    /// Each rule of the checks 1, 3, 4, 5, 6 and 7 fires on its own violation.
    #[test]
    fn each_rule_reports_its_violation() {
        let project = Project(Find::Project("id"));
        let cases: Vec<(Entry, AuthKind, &str)> = vec![
            (
                bot("Forms.records.read", Get, "/p", project, Read),
                AuthKind::BotOrUser,
                "invalid operation key",
            ),
            (
                bot("forms.records.fly", Get, "/p", project, Read),
                AuthKind::BotOrUser,
                "invalid operation key",
            ),
            (
                bot("forms.records.read/Bad", Get, "/p", project, Read),
                AuthKind::BotOrUser,
                "invalid operation key",
            ),
            (
                bot("forms.records.read", Get, "/p", project, Read),
                AuthKind::Jwt,
                "BOT exposure",
            ),
            (
                console("forms.records.read", Get, "/p", project, Read),
                AuthKind::BotOrUser,
                "BOT exposure",
            ),
            (
                bot("forms.records.read", Get, "/p", Public, Read),
                AuthKind::BotOrUser,
                "a bot route needs a scope",
            ),
            (
                console("forms.records.read", Get, "/p", Own, Read),
                AuthKind::None,
                "scope public",
            ),
            (
                console("forms.records.read", Get, "/p", Public, Read),
                AuthKind::Jwt,
                "scope public",
            ),
            (
                console("system.stats.read", Get, "/p", Own, Admin),
                AuthKind::Admin,
                "instance_admin",
            ),
            (
                console("forms.records.read", Get, "/p", InstanceAdmin, Admin),
                AuthKind::Jwt,
                "instance_admin",
            ),
            (
                bot("forms.records.read", Get, "/p", project, Read).oauth(),
                AuthKind::BotOrUser,
                "OAUTH exposure",
            ),
            (
                bot("account.me.read", Get, "/p", Own, Read).plugin(),
                AuthKind::BotOrUser,
                "PLUGIN exposure",
            ),
            (
                bot("forms.records.delete", Delete, "/p", project, Destructive).plugin(),
                AuthKind::BotOrUser,
                "PLUGIN exposure",
            ),
            (
                bot("forms.records.read", Get, "/api/site/v1/p", project, Read),
                AuthKind::BotOrUser,
                "SITE exposure",
            ),
        ];
        for (entry, auth, expected) in cases {
            let method = entry.method;
            let path = entry.path;
            let got = findings(&[entry], &[fact(method, path, auth)], &[]);
            assert!(got.iter().any(|l| l.contains(expected)), "{expected}: {got:?}");
        }
    }

    #[test]
    fn duplicates_and_mixed_risk_are_findings() {
        let project = Project(Find::Project("id"));
        let entries = [
            bot("forms.records.read", Get, "/p", project, Read),
            bot("forms.records.read", Get, "/q", project, Read),
            bot("forms.records.update", Put, "/p", project, Write),
            bot("forms.records.update/x", Patch, "/p", project, Destructive),
        ];
        let facts = [
            fact(Method::Get, "/p", AuthKind::BotOrUser),
            fact(Method::Get, "/q", AuthKind::BotOrUser),
            fact(Method::Put, "/p", AuthKind::BotOrUser),
            fact(Method::Patch, "/p", AuthKind::BotOrUser),
        ];
        let got = findings(&entries, &facts, &[]);
        assert!(
            got.iter()
                .any(|l| l.contains("operation key used twice: forms.records.read")),
            "{got:?}"
        );
        assert!(got.iter().any(|l| l.contains("do not share one risk")), "{got:?}");
    }

    /// An id takes the highest risk of its routes and the plugin flag of any route.
    #[test]
    fn ids_are_the_distinct_capabilities() {
        let project = Project(Find::Project("id"));
        let entries = [
            bot("forms.records.read", Get, "/p", project, Read).plugin(),
            bot("forms.records.read/list", Get, "/q", project, Read),
            bot("forms.records.update", Patch, "/p", project, Write),
        ];
        let ids = capability_ids(&entries);
        assert_eq!(ids.len(), 2);
        assert_eq!(
            ids.first().map(|id| (id.id, id.plugin_exposed)),
            Some(("forms.records.read", true))
        );
        assert_eq!(ids.get(1).map(|id| id.risk), Some(Write));
        // Only an id with plugin ops has a digest, and it changes with the op set.
        assert!(
            ids.first()
                .is_some_and(|id| id.plugin_ops_digest.as_deref().is_some_and(|d| d.len() == 71))
        );
        assert_eq!(ids.get(1).and_then(|id| id.plugin_ops_digest.clone()), None);
        let widened = [entries[0], entries[1].plugin(), entries[2]];
        assert_ne!(
            capability_ids(&widened)
                .first()
                .and_then(|id| id.plugin_ops_digest.clone()),
            ids.first().and_then(|id| id.plugin_ops_digest.clone())
        );
    }
}
