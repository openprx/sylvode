use super::{MIGRATIONS, RecordedMigration, migration_checksum, plan_migrations};
use std::collections::HashMap;

/// [`MIGRATIONS`] is generated from the `migrations/` directory at build time; this compares the
/// generated list with a fresh directory listing, so a registry that stops covering the directory
/// (a filter or an ordering change in `build.rs`) is caught.
#[test]
fn every_migration_file_is_registered() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".sql"))
        .collect();
    on_disk.sort();

    let registered: Vec<String> = MIGRATIONS.iter().map(|(name, _)| (*name).to_string()).collect();
    assert_eq!(
        registered, on_disk,
        "migrations/ and the generated MIGRATIONS must list the same files"
    );
}

#[test]
fn migrations_are_registered_in_file_name_order() {
    let mut sorted: Vec<&str> = MIGRATIONS.iter().map(|(name, _)| *name).collect();
    let registered = sorted.clone();
    sorted.sort_unstable();
    assert_eq!(registered, sorted, "MIGRATIONS must stay in file name order");
}

/// The ledger bootstrap has to be applicable before anything can be recorded, so it must be
/// the first entry and it must be safe to execute on every start.
#[test]
fn the_ledger_bootstrap_is_first_and_idempotent() {
    let (name, sql) = MIGRATIONS.first().expect("at least one migration is registered");
    assert_eq!(*name, "0000_schema_migrations.sql");
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS schema_migrations"));
}

/// A changed file is detected by its checksum, so the checksum has to be a pure function of the
/// content and has to separate two different files.
#[test]
fn migration_checksums_are_content_addressed() {
    assert_eq!(migration_checksum("select 1;"), migration_checksum("select 1;"));
    assert_ne!(migration_checksum("select 1;"), migration_checksum("select 2;"));
    assert_eq!(migration_checksum("").len(), 64);
}

const FILES: &[(&str, &str)] = &[
    ("0000_a.sql", "select 0;"),
    ("0001_b.sql", "select 1;"),
    ("0002_c.sql", "select 2;"),
    ("0003_d.sql", "select 3;"),
];

fn row(sql: &str, status: &str) -> RecordedMigration {
    RecordedMigration {
        checksum: migration_checksum(sql),
        status: status.to_string(),
    }
}

fn recorded(names: &[&str]) -> HashMap<String, RecordedMigration> {
    FILES
        .iter()
        .filter(|(name, _)| names.contains(name))
        .map(|(name, sql)| ((*name).to_string(), row(sql, "applied")))
        .collect()
}

#[test]
fn an_empty_ledger_plans_every_file_in_order() {
    let pending = plan_migrations(FILES, &HashMap::new()).expect("plan");
    assert_eq!(pending, FILES.to_vec());
}

#[test]
fn a_fully_recorded_ledger_plans_nothing() {
    let ledger = recorded(&["0000_a.sql", "0001_b.sql", "0002_c.sql", "0003_d.sql"]);
    assert!(plan_migrations(FILES, &ledger).expect("plan").is_empty());
}

#[test]
fn a_file_numbered_below_a_recorded_one_still_runs() {
    let ledger = recorded(&["0000_a.sql", "0002_c.sql", "0003_d.sql"]);
    let pending = plan_migrations(FILES, &ledger).expect("plan");
    assert_eq!(pending, vec![("0001_b.sql", "select 1;")]);
}

#[test]
fn a_changed_recorded_file_is_refused() {
    let mut ledger = recorded(&["0000_a.sql", "0001_b.sql"]);
    ledger.insert("0001_b.sql".to_string(), row("select 99;", "applied"));
    let err = plan_migrations(FILES, &ledger).expect_err("checksum mismatch must refuse");
    let text = err.to_string();
    assert!(text.contains("0001_b.sql changed after it was applied"), "{text}");
    assert!(
        text.contains(&migration_checksum("select 99;")),
        "names the recorded checksum: {text}"
    );
    assert!(
        text.contains(&migration_checksum("select 1;")),
        "names the shipped checksum: {text}"
    );
    assert!(text.contains("restore the pre-upgrade backup") && text.contains("never edit schema_migrations"));
}

#[test]
fn a_recorded_file_that_is_not_shipped_is_refused() {
    let mut ledger = recorded(&["0000_a.sql"]);
    ledger.insert("0099_gone.sql".to_string(), row("select 9;", "applied"));
    let err = plan_migrations(FILES, &ledger).expect_err("unshipped record must refuse");
    let text = err.to_string();
    assert!(text.contains("0099_gone.sql is recorded but not shipped"), "{text}");
    assert!(text.contains("Deploy the newer build") && text.contains("never edit schema_migrations"));
}

#[test]
fn a_failed_row_is_retried_whatever_its_checksum() {
    let mut ledger = recorded(&["0000_a.sql", "0002_c.sql"]);
    ledger.insert("0001_b.sql".to_string(), row("select 99;", "failed"));
    let pending = plan_migrations(FILES, &ledger).expect("plan");
    assert_eq!(pending, vec![("0001_b.sql", "select 1;"), ("0003_d.sql", "select 3;")]);
}

#[test]
fn a_failed_row_without_a_shipped_file_is_ignored() {
    let mut ledger = recorded(&["0000_a.sql"]);
    ledger.insert("0099_gone.sql".to_string(), row("select 9;", "failed"));
    assert!(plan_migrations(FILES, &ledger).is_ok());
}
/// Routes are authenticated one by one with `route_layer`, so forgetting one is invisible
/// until somebody calls it. Four proposal reads were unauthenticated that way, two of which
/// finalized voting and wrote decisions. The router source is parsed here and every route
/// without a layer has to be on the list of endpoints that are public on purpose.
#[test]
fn only_the_intended_routes_are_reachable_without_authentication() {
    /// Endpoints that must answer without a session, and why.
    const PUBLIC_ROUTES: &[&str] = &[
        // Public site API: its own principal (visitor cookie, operator session) and layer stack
        // (origin + X-SV-Site check, own CORS/CSP, body and rate limits); console credentials refused.
        "/api/site/v1/{slug}/auth/logout",
        "/api/site/v1/{slug}/auth/operator/login",
        "/api/site/v1/{slug}/bindings/{binding}/fields",
        "/api/site/v1/{slug}/bindings/{binding}/records",
        "/api/site/v1/{slug}/bindings/{binding}/records/{id}",
        "/api/site/v1/{slug}/manifest",
        "/api/site/v1/{slug}/pages",
        // Signed, expiring download links; the handler verifies the HMAC itself.
        "/api/v1/uploads/signatures/{file_name}/download",
        "/api/v1/form-attachments/{attachment_id}/download",
        // Liveness and readiness probes.
        "/health",
        "/ready",
        // Credential entry points; there is no session to present yet.
        "/api/v1/auth/register",
        "/api/v1/auth/login",
        "/api/v1/auth/refresh",
        // First-run check: answers one boolean, whether any person exists yet, so the setup page
        // can be offered before there is a session.
        "/api/v1/auth/setup-status",
        // WebSocket upgrade: authenticates via the one-time collab ticket the handler itself
        // consumes atomically (`ADR-0007`), never a Bearer/cookie token — a browser WebSocket
        // cannot set an `Authorization` header, so this route deliberately carries no
        // `bot_or_user_auth_middleware` layer. The ticket is bound to a UUID client id and an
        // origin, and the upgrade revalidates scoped document access.
        "/api/v2/flow/collab/ws",
        // Scoped public links authenticate against a hashed, unexpired, unrevoked token
        // and return only sanitized content or assets currently referenced by that Page.
        "/api/v2/flow/public-shares/{token}",
        "/api/v2/flow/public-shares/{token}/assets/{asset_id}",
        "/api/v2/flow/public-shares/{token}/body-assets/{asset_id}",
    ];

    let mut unauthenticated: Vec<String> = Vec::new();
    let mut block: Option<String> = None;

    let finish = |block: &str, out: &mut Vec<String>| {
        if block.contains("route_layer") {
            return;
        }
        if let Some(path) = block.split('"').nth(1) {
            out.push(path.to_string());
        }
    };

    // Every router function is one method chain
    // whose calls start at this indentation; a block ends at the next call of the chain or at
    // the end of the function.
    for line in crate::router::ROUTER_SOURCES
        .iter()
        .flat_map(|(_, source)| source.lines())
    {
        let starts_route = line.starts_with("        .route(");
        let ends_block = line.starts_with("        .") || line == "}";
        if (starts_route || ends_block)
            && let Some(previous) = block.take()
        {
            finish(&previous, &mut unauthenticated);
        }
        if starts_route {
            block = Some(String::new());
        }
        // A comment between two calls may mention `route_layer`; it is not a layer.
        if line.trim_start().starts_with("//") {
            continue;
        }
        if let Some(current) = block.as_mut() {
            current.push_str(line);
            current.push('\n');
        }
    }
    if let Some(previous) = block.take() {
        finish(&previous, &mut unauthenticated);
    }

    assert!(
        !unauthenticated.is_empty(),
        "the router parser found nothing; it no longer matches the router files"
    );
    let mut expected: Vec<String> = PUBLIC_ROUTES.iter().map(|path| (*path).to_string()).collect();
    expected.sort();
    let mut found = unauthenticated;
    found.sort();
    assert_eq!(
        found, expected,
        "a route was registered without an authentication layer; add the layer, or add the \
         path to PUBLIC_ROUTES with the reason it is public"
    );
}
