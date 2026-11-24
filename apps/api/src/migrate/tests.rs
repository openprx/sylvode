use super::{
    MIGRATION_ADOPTION_CUTOFF, MIGRATION_PROBES, MIGRATIONS, MigrationOptions, SchemaProbe, migration_checksum,
};
use platform::config::MigrationsConfig;

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

/// The cutoff decides which files an existing database claims without executing them.
/// It must name a real migration, and everything after it must actually run.
#[test]
fn the_adoption_cutoff_names_a_registered_migration() {
    assert!(
        MIGRATIONS.iter().any(|(name, _)| *name == MIGRATION_ADOPTION_CUTOFF),
        "adoption cutoff must be a registered migration"
    );
    let replayed: Vec<&str> = MIGRATIONS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| *name > MIGRATION_ADOPTION_CUTOFF)
        .collect();
    assert_eq!(
        replayed,
        vec![
            "0048_delivery_lease_and_pickup_indexes.sql",
            "0049_agent_invocation_duplicate_tagging.sql",
            "0050_proposal_workspace_scope.sql",
            "0051_bot_operation_logs.sql",
            "0052_drop_connectors_and_agent_invocations.sql",
            "0053_drop_event_outbox.sql",
            "0054_flow_data_layer.sql",
            "0055_flow_import_jobs.sql",
            "0056_flow_objects_parent_project_invariant.sql",
            "0057_flow_objects_parent_project_scope_index.sql",
            "0058_flow_search_index.sql",
            "0059_flow_navigator_root.sql",
            "0060_flow_collections_core.sql",
            "0061_flow_collections_query_indexes.sql",
            "0062_flow_forms_bridge.sql",
            "0063_flow_v08_hardening.sql",
            "0064_flow_v08_replay_requests.sql",
            "0065_flow_v08_operation_idempotency.sql",
            "0066_flow_v08_object_retention.sql",
            "0067_flow_package_jobs.sql",
            "0068_flow_authz_revocations.sql",
            "0069_flow_v08_rollback_control.sql",
            "0070_bot_operation_logs_surface_check.sql",
            "0071_flow_conversion_job_error_reason.sql",
            "0072_flow_conversion_job_started_at.sql",
            "0073_user_session_version.sql",
            "0074_flow_comments.sql",
            "0075_flow_share_links.sql",
            "0076_flow_page_appearance.sql",
            "0077_flow_public_share_rate.sql",
            "0078_flow_scope_expansion.sql",
            "0079_flow_scope_constraint_cutover.sql",
            "0080_flow_scope_secondary_ownership.sql",
            "0081_flow_scope_typed_ownership.sql",
            "0082_flow_scope_relations_mount_ancestry.sql",
            "0083_flow_export_authority_snapshot.sql",
            "0084_flow_legacy_export_authority.sql",
            "0085_flow_ticket_authorization_notifications.sql",
            "0086_flow_purge_tombstone_receipts.sql",
            "0087_flow_reference_source_scope.sql",
            "0088_flow_scope_import_ownership.sql",
            "0089_flow_export_scope_selection.sql",
            "0090_flow_conversion_source_scope.sql",
            "0091_flow_operation_scope_ownership.sql",
            "0092_ai_participant_config_version.sql",
            "0093_flow_page_body_assets.sql",
            "0094_flow_child_page_reservations.sql",
            "0095_flow_logical_blocks.sql",
            "0096_flow_page_block_transfers.sql",
            "0097_flow_portable_page_packages.sql",
            "0098_flow_portable_graph_packages.sql",
            "0099_form_plugin_packages.sql",
            "0100_form_plugin_layouts.sql",
            "0101_form_plugin_library.sql",
            "0102_flow_integrity_cross_workspace_attempts.sql",
            "0103_form_plugin_extensions.sql",
            "0104_project_resource_kinds.sql",
            "0105_token_bot_entity_type.sql",
            "0106_flow_capabilities_default_on.sql",
            "0107_flow_block_registration_progress.sql",
            "0108_flow_block_checkpoint_digest.sql",
            "0109_oauth_authorization_server.sql",
            "0110_runner_task_protocol.sql",
            "0111_alert_chain.sql",
            "0112_bot_connections.sql",
            "0113_ai_task_chain.sql",
            "0114_multi_bot_connections.sql",
            "0115_flow_default_on.sql",
            "0116_webhook_delivery_duration.sql",
            "0117_sprint_rules.sql",
            "0118_label_rules.sql",
            "0119_plugin_single_active_version.sql",
            "0120_email_outbox.sql",
            "0121_milestones.sql",
            "0122_bot_project_access.sql",
            "0123_work_item_page_links.sql",
            "0124_comment_mentions.sql",
            "0125_oauth_client_scopes.sql",
            "0126_retire_unused_columns.sql",
            "0127_retire_legacy_objects.sql",
            "0130_tmux_cli_codex.sql",
            "0131_drop_one_tmux_per_cli.sql",
        ],
        "everything past the cutoff re-runs on an adopted database and must be idempotent"
    );
}

/// Adoption records a checksum without executing the file, so the checksum has to be a pure
/// function of the content and has to separate two different files.
#[test]
fn migration_checksums_are_content_addressed() {
    assert_eq!(migration_checksum("select 1;"), migration_checksum("select 1;"));
    assert_ne!(migration_checksum("select 1;"), migration_checksum("select 2;"));
    assert_eq!(migration_checksum("").len(), 64);
}

/// The post-cutoff files that intentionally drop relations. Everything else past the cutoff
/// must stay replay-safe.
const RETIREMENT_MIGRATIONS: [&str; 4] = [
    "0052_drop_connectors_and_agent_invocations.sql",
    "0053_drop_event_outbox.sql",
    "0114_multi_bot_connections.sql",
    "0127_retire_legacy_objects.sql",
];

/// Non-destructive post-cutoff migrations must remain safe to replay. The explicitly
/// destructive retirement migrations are checked separately.
#[test]
fn the_delivery_migrations_never_delete_rows() {
    for (name, sql) in MIGRATIONS.iter().filter(|(name, _)| *name > MIGRATION_ADOPTION_CUTOFF) {
        if RETIREMENT_MIGRATIONS.contains(name) {
            continue;
        }
        let upper = sql.to_uppercase();
        assert!(
            !upper.contains("DELETE FROM"),
            "{name} must not delete rows: it re-runs on every adopted database"
        );
        assert!(
            !upper.contains("DROP TABLE"),
            "{name} must not drop tables: it re-runs on every adopted database"
        );
    }
}

/// A post-cutoff migration is executed whenever the ledger has no row for it, and that can
/// happen after the relations it touches were already dropped: a database restored without
/// its ledger, or one retrying the file after a single failed run. Referencing a dropped
/// relation unguarded then fails on every start and never reaches the migration that drops
/// it, which is an unrecoverable restart loop rather than a bad run.
#[test]
fn post_cutoff_migrations_guard_the_relations_a_retirement_migration_drops() {
    let dropped = [
        "connectors",
        "agent_invocation_tool_calls",
        "agent_invocations",
        "event_outbox",
        "project_execution_settings",
    ];
    for (name, sql) in MIGRATIONS.iter().filter(|(name, _)| *name > MIGRATION_ADOPTION_CUTOFF) {
        if RETIREMENT_MIGRATIONS.contains(name) {
            continue;
        }
        for relation in dropped {
            // Whole identifiers only: `oauth_event_outbox` is not the retired `event_outbox`.
            let mentioned = sql.match_indices(relation).any(|(at, _)| {
                !sql[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|prev| prev.is_ascii_alphanumeric() || prev == '_')
            });
            if !mentioned {
                continue;
            }
            assert!(
                sql.contains(&format!("to_regclass('{relation}')")),
                "{name} touches {relation}, which a later migration drops, so every statement \
                 reaching it must sit behind a to_regclass('{relation}') guard"
            );
        }
    }
}

#[test]
fn the_retirement_migrations_drop_only_the_confirmed_relations() {
    for (name, dropped) in [
        (
            "0052_drop_connectors_and_agent_invocations.sql",
            ["agent_invocation_tool_calls", "agent_invocations", "connectors"].as_slice(),
        ),
        ("0053_drop_event_outbox.sql", ["event_outbox"].as_slice()),
        (
            "0114_multi_bot_connections.sql",
            ["project_execution_settings"].as_slice(),
        ),
        (
            "0127_retire_legacy_objects.sql",
            ["pages", "form_plugin_layouts"].as_slice(),
        ),
    ] {
        let sql = MIGRATIONS
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, sql)| *sql)
            .expect("retirement migration must be registered");
        for relation in dropped {
            assert!(
                sql.contains(&format!("DROP TABLE IF EXISTS {relation}")),
                "{name} must drop {relation} idempotently"
            );
        }
        assert_eq!(
            sql.matches("DROP TABLE").count(),
            dropped.len(),
            "{name} must not drop an unrelated table"
        );
    }
}

/// Every migration needs a probe, otherwise an existing database either replays it (which the
/// early non idempotent files do not survive) or, worse, a future migration falls back on a
/// probe belonging to another file. The two lists are kept in lockstep here.
#[test]
fn every_migration_has_a_schema_probe() {
    let migrations: Vec<&str> = MIGRATIONS.iter().map(|(name, _)| *name).collect();
    let probed: Vec<&str> = MIGRATION_PROBES.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        migrations, probed,
        "MIGRATIONS and MIGRATION_PROBES must list the same files in the same order"
    );
}

/// A probe has to produce a statement, except for the files that create nothing and are
/// executed rather than adopted.
#[test]
fn only_rerunnable_probes_have_no_statement() {
    let without: Vec<&str> = MIGRATION_PROBES
        .iter()
        .filter(|(_, probe)| matches!(probe, SchemaProbe::Rerunnable))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        without,
        vec![
            "0014_proposal_comments_schema_compat.sql",
            "0021_fix_cascade.sql",
            "0035_project_type_two_lanes.sql",
            "0105_token_bot_entity_type.sql",
            "0125_oauth_client_scopes.sql",
            "0127_retire_legacy_objects.sql",
        ],
        "a migration may only skip its probe when re-executing it is a no-op"
    );
}

/// 0106 re-runs whenever the ledger has no row for it, so the one-time switch-on must be
/// claimed through its marker, may only raise values, and must leave the capabilities that
/// stay off alone. Comment lines are stripped so the explanation in the file cannot satisfy
/// or trip the assertions.
#[test]
fn flow_capability_default_migration_runs_once_and_only_raises() {
    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0106_flow_capabilities_default_on.sql")
        .map(|(_, sql)| *sql)
        .expect("0106 must be registered");
    let statements: String = sql
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    let update = statements
        .split_once("UPDATE flow_runtime_capabilities")
        .map(|(_, rest)| rest)
        .expect("0106 updates the singleton row");

    assert!(
        statements.contains("ON CONFLICT (name) DO NOTHING") && update.contains("AND EXISTS (SELECT 1 FROM claimed)"),
        "the row update must run only in the execution that claims the rollout marker"
    );
    assert!(
        update.contains("minimum_reader_version = GREATEST(minimum_reader_version, 5)"),
        "the reader floor may only be raised"
    );
    assert!(
        !update.to_lowercase().contains("false"),
        "the one-time update may only switch capabilities on"
    );
    for column in [
        "personal_provisioning",
        "personal_writes",
        "external_mounts",
        "personal_forms_conversion",
        "block_transfers_enabled",
        "portable_page_packages_enabled",
        "portable_graph_packages_enabled",
    ] {
        assert!(
            update.contains(&format!("{column} = true")),
            "{column} must be switched on for existing databases"
        );
        assert!(
            statements.contains(&format!("ALTER COLUMN {column} SET DEFAULT true")),
            "{column} must default on for new rows"
        );
    }
    for column in [
        "scope_native_legacy",
        "personal_package_imports",
        "personal_scope_package_exports",
        "cross_workspace_forms_conversion",
    ] {
        assert!(
            !statements.contains(column),
            "{column} belongs to 0115 and must not be touched here"
        );
    }
}

/// 0115 re-runs whenever the ledger has no row for it: the capability switch-on must be claimed
/// through its marker and may only raise values, and the backfill must never overwrite a row.
#[test]
fn flow_default_on_migration_runs_once_and_never_overwrites() {
    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0115_flow_default_on.sql")
        .map(|(_, sql)| *sql)
        .expect("0115 must be registered");
    let statements: String = sql
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    let update = statements
        .split_once("UPDATE flow_runtime_capabilities")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("CREATE OR REPLACE FUNCTION"))
        .map(|(update, _)| update)
        .expect("0115 updates the singleton row");
    assert!(
        statements.contains("ON CONFLICT (name) DO NOTHING") && update.contains("AND EXISTS (SELECT 1 FROM claimed)"),
        "the row update must run only in the execution that claims the rollout marker"
    );
    assert!(
        !update.to_lowercase().contains("false"),
        "the one-time update may only switch capabilities on"
    );
    assert!(
        !statements.contains("UPDATE flow_workspace_settings"),
        "an existing settings row is never changed"
    );
    assert!(
        statements.matches("ON CONFLICT (workspace_id) DO NOTHING").count() == 2,
        "the trigger and the backfill both leave an existing row alone"
    );
}

#[test]
fn only_retired_delivery_migrations_are_superseded() {
    let superseded: Vec<&str> = MIGRATION_PROBES
        .iter()
        .filter(|(_, probe)| matches!(probe, SchemaProbe::Superseded))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        superseded,
        vec![
            "0026_connectors_invocations.sql",
            "0028_invocation_tool_calls.sql",
            "0032_print_device_connector_kinds.sql",
            "0048_delivery_lease_and_pickup_indexes.sql",
            "0049_agent_invocation_duplicate_tagging.sql"
        ]
    );
}

/// Probes must resolve relations through search_path. A hardcoded `public.` prefix reports
/// "absent" on a deployment that does not use the public schema, which sends the runner into
/// replaying the early non idempotent migrations.
#[test]
fn probes_do_not_hardcode_the_public_schema() {
    for (name, probe) in MIGRATION_PROBES {
        let Some(statement) = probe.statement() else {
            continue;
        };
        let rendered = format!("{statement:?}");
        assert!(
            !rendered.contains("public."),
            "{name} probe must not hardcode the public schema"
        );
    }
}

#[test]
fn migration_escape_hatches_are_off_until_the_file_asks_for_them() {
    let quiet = MigrationOptions::from_config(MigrationsConfig::default());
    assert!(!quiet.replay);
    assert!(!quiet.continue_on_error);

    // Each switch is read from its own key. Asserted one at a time so a swapped mapping
    // cannot pass by setting both.
    let replaying = MigrationOptions::from_config(MigrationsConfig {
        replay: true,
        continue_on_error: false,
    });
    assert!(replaying.replay);
    assert!(!replaying.continue_on_error);

    let tolerant = MigrationOptions::from_config(MigrationsConfig {
        replay: false,
        continue_on_error: true,
    });
    assert!(!tolerant.replay);
    assert!(tolerant.continue_on_error);
}

/// Routes are authenticated one by one with `route_layer`, so forgetting one is invisible
/// until somebody calls it. Four proposal reads were unauthenticated that way, two of which
/// finalized voting and wrote decisions. The router source is parsed here and every route
/// without a layer has to be on the list of endpoints that are public on purpose.
#[test]
fn only_the_intended_routes_are_reachable_without_authentication() {
    /// Endpoints that must answer without a session, and why.
    const PUBLIC_ROUTES: &[&str] = &[
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
        // `bot_or_user_auth_middleware` layer.
        "/api/v1/collab/ws",
        // Anonymous Flow page link. The handler hashes the token, checks expiry and
        // revocation, returns only that page's sanitized text, and rate-limits by address.
        "/api/v1/flow/public-shares/{token}",
        // Icon or cover for that same link. The handler returns bytes only while the page's
        // current appearance still references the asset.
        "/api/v1/flow/public-shares/{token}/assets/{asset_id}",
        // Scoped WebSocket upgrades consume an origin-bound, one-time ticket before
        // revalidating scoped document access.
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
