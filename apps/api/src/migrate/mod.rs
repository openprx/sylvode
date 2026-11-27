//! The migration runner: ordered migration files, the ledger that records them, and the startup
//! checks that follow.

use platform::config::MigrationsConfig;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// Newest migration an existing database may claim without executing it.
///
/// Frozen on purpose. Everything past it is executed on every database that has no ledger row for
/// it, which is what keeps a future migration from being adopted by accident because its probe
/// happens to match an object some other file created. The files past the cutoff are
/// idempotent and a test enforces that they stay that way.
const MIGRATION_ADOPTION_CUTOFF: &str = "0047_form_attachment_media_metadata.sql";

/// Configuration key of the escape hatch that re-executes every migration once, reporting
/// failures without aborting.
///
/// Safe to use on a live database. A failure never downgrades a ledger row that already records
/// a success, so a replay of the non idempotent early migrations ("relation already exists") no
/// longer turns the next ordinary start into a permanent failure. Named here only so the warning
/// it prints tells the operator which line of the file to turn back off.
const MIGRATION_REPLAY_KEY: &str = "migrations.replay";

/// Configuration key of the escape hatch that starts the service even though a migration failed
/// or the schema check found a gap.
///
/// The degraded path an operator needs to get the service up and inspect it. The failure is
/// recorded and logged; nothing is skipped silently, and the next ordinary start retries.
const MIGRATION_CONTINUE_ON_ERROR_KEY: &str = "migrations.continue_on_error";

/// Advisory lock namespace for the migration runner. The first key separates this lock from any
/// other advisory lock in the system, the second one names the runner within that namespace.
const MIGRATION_LOCK_NAMESPACE: i32 = 0x0F09_2026;
const MIGRATION_LOCK_ID: i32 = 1;

/// Evidence that one migration is already present in a database created before the ledger.
///
/// The previous revision took this decision once for the whole list with
/// `to_regclass('public.users')`: every database that had a `users` table claimed all migrations
/// up to the cutoff. A half built database - 0001 succeeded years ago, 0030 kept failing and the
/// pre-ledger runner only warned - was therefore marked complete and its missing tables were
/// never created again. Adoption is now decided per file, so a gap heals by executing that one
/// file, and the same probes double as the post-run schema assertion.
///
/// A probe that wrongly reports "absent" only costs one execution of an idempotent file. A probe
/// that wrongly reports "present" would skip a migration forever, so every probe names an object
/// that its own migration creates.
#[derive(Clone, Copy)]
enum SchemaProbe {
    /// Table, index or view. Resolved through `search_path`, so a deployment that does not use
    /// the `public` schema is judged against its own schema instead of being told nothing exists.
    Relation(&'static str),
    /// A relation intentionally removed by a destructive migration.
    RelationAbsent(&'static str),
    /// Column on a table, by table and column name.
    Column(&'static str, &'static str),
    /// Column intentionally removed by a destructive migration, by table and column name.
    ColumnAbsent(&'static str, &'static str),
    /// Label on an enum type.
    EnumLabel(&'static str, &'static str),
    /// Named constraint whose definition contains a marker the migration introduced.
    ConstraintContains(&'static str, &'static str, &'static str),
    /// A constraint marker and a trigger must both exist. Used when one migration creates the
    /// static invariant and the mechanism that preserves it for future parent rows.
    ConstraintAndTrigger(&'static str, &'static str, &'static str, &'static str),
    /// Non-internal trigger on a table, by table and trigger name.
    Trigger(&'static str, &'static str),
    /// Complete independent block-sharing registry, private anchors and lifecycle fences.
    BlockSharing,
    PageBlockTransfers,
    CrossWorkspaceIntegrity,
    /// No object is created that could be probed, but re-executing the file is a no-op, so it is
    /// executed rather than adopted.
    Rerunnable,
    /// A later migration intentionally removed every object this migration created. Adoption
    /// still executes it in order, while post-run verification belongs to the removing migration.
    Superseded,
}

impl SchemaProbe {
    /// Statement returning a single `present` boolean, or `None` for [`SchemaProbe::Rerunnable`].
    ///
    /// Every identifier travels as a bind parameter, so nothing here is concatenated into SQL.
    fn statement(self) -> Option<Statement> {
        match self {
            Self::Relation(name) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_regclass($1) IS NOT NULL AS present",
                vec![name.into()],
            )),
            Self::RelationAbsent(name) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_regclass($1) IS NULL AS present",
                vec![name.into()],
            )),
            Self::Column(table, column) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_attribute
                        WHERE attrelid = to_regclass($1)
                          AND attname = $2
                          AND attnum > 0
                          AND NOT attisdropped
                    ) AS present
                ",
                vec![table.into(), column.into()],
            )),
            Self::ColumnAbsent(table, column) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT NOT EXISTS (
                        SELECT 1 FROM pg_attribute
                        WHERE attrelid = to_regclass($1)
                          AND attname = $2
                          AND attnum > 0
                          AND NOT attisdropped
                    ) AS present
                ",
                vec![table.into(), column.into()],
            )),
            Self::Trigger(table, trigger) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_trigger
                        WHERE tgrelid = to_regclass($1)
                          AND tgname = $2
                          AND NOT tgisinternal
                    ) AS present
                ",
                vec![table.into(), trigger.into()],
            )),
            Self::EnumLabel(type_name, label) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_enum
                        WHERE enumtypid = to_regtype($1)
                          AND enumlabel = $2
                    ) AS present
                ",
                vec![type_name.into(), label.into()],
            )),
            Self::ConstraintContains(table, constraint, marker) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT EXISTS (
                        SELECT 1 FROM pg_constraint
                        WHERE conrelid = to_regclass($1)
                          AND conname = $2
                          AND position($3 IN pg_get_constraintdef(oid)) > 0
                    ) AS present
                ",
                vec![table.into(), constraint.into(), marker.into()],
            )),
            Self::ConstraintAndTrigger(table, constraint, marker, trigger) => Some(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                        SELECT EXISTS (
                            SELECT 1 FROM pg_constraint
                            WHERE conrelid = to_regclass($1)
                              AND conname = $2
                              AND position($3 IN pg_get_constraintdef(oid)) > 0
                        ) AND EXISTS (
                            SELECT 1 FROM pg_trigger
                            WHERE tgrelid = to_regclass('workspaces')
                              AND tgname = $4
                              AND NOT tgisinternal
                        ) AS present
                    ",
                vec![table.into(), constraint.into(), marker.into(), trigger.into()],
            )),
            Self::PageBlockTransfers => Some(Statement::from_string(DbBackend::Postgres,
                "SELECT to_regclass('flow_page_transfer_pending') IS NOT NULL
                  AND EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid=to_regclass('flow_page_block_transfers') AND attname='copied_placements' AND atttypid='jsonb'::regtype AND NOT attisdropped)
                  AND EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid=to_regclass('flow_runtime_capabilities') AND attname='block_transfers_enabled' AND atttypid='boolean'::regtype AND attnotnull AND NOT attisdropped)
                  AND EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid=to_regclass('flow_page_block_transfers') AND pg_get_constraintdef(oid)='UNIQUE (actor_id, source_object_id, prepare_key)')
                  AS present".to_owned(),
            )),
            Self::CrossWorkspaceIntegrity => Some(Statement::from_string(
                DbBackend::Postgres,
                "SELECT EXISTS (
                   SELECT 1 FROM pg_trigger
                   WHERE tgrelid = to_regclass('flow_integrity_records')
                     AND tgname = 'flow_integrity_bind_owner'
                     AND NOT tgisinternal
                     AND tgenabled IN ('O', 'A')
                     AND position('details_redacted' IN pg_get_triggerdef(oid)) > 0
                     AND position('requesting_workspace_id' IN pg_get_functiondef(tgfoid)) > 0
                     AND position('referenced_workspace_id' IN pg_get_functiondef(tgfoid)) > 0
                 ) AS present",
            )),
            Self::BlockSharing => Some(Statement::from_sql_and_values(DbBackend::Postgres,
                r"SELECT
 (SELECT bool_and(to_regclass(name) IS NOT NULL) FROM unnest($1::text[]) AS objects(name))
 AND (SELECT bool_and(EXISTS(SELECT 1 FROM pg_attribute WHERE attrelid=to_regclass(required.relation_name) AND attname=required.column_name AND attnum>0 AND NOT attisdropped)) FROM unnest($2::text[],$3::text[]) AS required(relation_name,column_name))
 AND (SELECT bool_and(EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid=to_regclass(required.relation_name) AND tgname=required.trigger_name AND NOT tgisinternal)) FROM unnest($4::text[],$5::text[]) AS required(relation_name,trigger_name))
 AND (SELECT bool_and(EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid=to_regclass(required.relation_name) AND position(required.marker IN pg_get_constraintdef(oid))>0)) FROM unnest($6::text[],$7::text[]) AS required(relation_name,marker)) AS present",
                vec![
                    vec!["flow_logical_blocks","flow_block_placements","flow_block_checkpoints","flow_block_shares","flow_block_share_members","flow_block_share_placements","flow_block_share_audiences","flow_block_share_asset_aliases","flow_block_share_receipts","flow_block_comment_anchors","flow_block_alias_fences","flow_block_one_active_logical","flow_block_one_active_physical","flow_block_placement_target"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                    vec!["flow_block_shares","flow_block_shares","flow_block_shares","flow_block_shares","flow_block_comment_anchors","flow_block_comment_anchors","flow_block_comment_anchors","flow_block_alias_fences","flow_block_share_audiences"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                    vec!["publisher_user_id","root_logical_id","source_root_cid","projection_hash","logical_id","origin_generation","origin_placement_id","ever_retired","placement_id"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                    vec!["flow_objects","flow_objects","flow_scopes","projects","workspaces","flow_block_placements","flow_block_placements"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                    vec!["flow_block_source_object_archive","flow_block_source_object_delete","flow_block_source_scope_archive","flow_block_target_project_delete","flow_block_target_workspace_delete","flow_block_alias_retire","flow_block_alias_prune"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                    vec!["flow_block_shares","flow_block_share_placements","flow_block_share_asset_aliases","flow_block_share_audiences"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                    vec!["FOREIGN KEY (root_logical_id)","UNIQUE (share_id)","FOREIGN KEY (source_scope_id, source_page_id, asset_id)","FOREIGN KEY (placement_id, share_id, workspace_id, project_key)"].into_iter().map(str::to_owned).collect::<Vec<_>>().into(),
                ])),
            Self::Rerunnable | Self::Superseded => None,
        }
    }
}

/// One probe per migration, in the same order as [`MIGRATIONS`], including the ledger bootstrap.
/// A test keeps the two lists aligned so a new migration cannot be added without deciding how an
/// existing database recognises it.
const MIGRATION_PROBES: &[(&str, SchemaProbe)] = &[
    ("0000_schema_migrations.sql", SchemaProbe::Relation("schema_migrations")),
    ("0001_init.sql", SchemaProbe::Relation("scheduled_jobs")),
    ("0002_users.sql", SchemaProbe::Column("users", "name")),
    ("0003_labels.sql", SchemaProbe::Relation("work_item_labels")),
    ("0004_sprints.sql", SchemaProbe::Column("work_items", "sprint_id")),
    ("0005_webhooks.sql", SchemaProbe::Relation("webhook_deliveries")),
    (
        "0006_notifications.sql",
        SchemaProbe::Column("notifications", "metadata"),
    ),
    (
        "0007_fulltext_search.sql",
        SchemaProbe::Column("comments", "search_vector"),
    ),
    ("0008_admin_user_fields.sql", SchemaProbe::Column("users", "is_active")),
    (
        "0009_notifications_schema_compat.sql",
        SchemaProbe::Relation("idx_notifications_user_unread"),
    ),
    (
        "0010_issue_activity_notifications_compat.sql",
        SchemaProbe::Column("notifications", "workspace_id"),
    ),
    (
        "0011_bot_user_and_webhook_fields.sql",
        SchemaProbe::Column("webhooks", "bot_user_id"),
    ),
    (
        "0012_governance_phase1.sql",
        SchemaProbe::Relation("proposal_issue_links"),
    ),
    (
        "0013_governance_work_items_fields.sql",
        SchemaProbe::Column("work_items", "proposal_id"),
    ),
    // Reshapes columns 0012 may already have created, so every object it could be probed on
    // exists before it runs. Every branch is guarded and the unguarded statements are
    // `SET NOT NULL` and idempotent updates, so it is executed rather than adopted.
    ("0014_proposal_comments_schema_compat.sql", SchemaProbe::Rerunnable),
    ("0015_governance_phase2.sql", SchemaProbe::Relation("appeals")),
    (
        "0016_governance_phase3.sql",
        SchemaProbe::Relation("governance_audit_logs"),
    ),
    (
        "0017_governance_phase3_hardening.sql",
        SchemaProbe::Relation("uq_trust_score_logs_event_scope"),
    ),
    (
        "0018_governance_phase3_templates.sql",
        SchemaProbe::Column("proposals", "template_id"),
    ),
    (
        "0019_governance_cycle_template_rapid.sql",
        SchemaProbe::EnumLabel("cycle_template", "rapid"),
    ),
    ("0020_ai_tasks.sql", SchemaProbe::Relation("ai_task_events")),
    // Only rewrites foreign keys that are not already cascading, and every branch is guarded, so
    // there is nothing to probe and nothing to lose by executing it again.
    ("0021_fix_cascade.sql", SchemaProbe::Rerunnable),
    ("0022_bot_tokens.sql", SchemaProbe::Relation("workspace_bots")),
    (
        "0023_work_item_identifier.sql",
        SchemaProbe::Column("work_items", "sequence_number"),
    ),
    ("0024_workflow_config.sql", SchemaProbe::Relation("workflow_states")),
    (
        "0025_project_types_resources.sql",
        SchemaProbe::Relation("project_resources"),
    ),
    ("0026_connectors_invocations.sql", SchemaProbe::Superseded),
    ("0027_check_results.sql", SchemaProbe::Relation("check_results")),
    ("0028_invocation_tool_calls.sql", SchemaProbe::Superseded),
    (
        "0029_scenario_templates.sql",
        SchemaProbe::Relation("scenario_templates"),
    ),
    (
        "0030_universal_forms.sql",
        SchemaProbe::Relation("form_record_field_index"),
    ),
    (
        "0031_business_events_outbox_inbox.sql",
        SchemaProbe::Relation("event_inbox"),
    ),
    ("0032_print_device_connector_kinds.sql", SchemaProbe::Superseded),
    ("0033_plugins_wasm.sql", SchemaProbe::Relation("plugin_invocations")),
    (
        "0034_user_profile_preferences.sql",
        SchemaProbe::Column("users", "notification_prefs"),
    ),
    // Seeds the two product lanes with ON CONFLICT DO UPDATE and retires the legacy keys; running
    // it again on a database that already has them changes nothing.
    ("0035_project_type_two_lanes.sql", SchemaProbe::Rerunnable),
    (
        "0036_universal_forms_schema_version_archive.sql",
        SchemaProbe::Relation("idx_form_field_index_field_id"),
    ),
    ("0037_form_attachments.sql", SchemaProbe::Relation("form_attachments")),
    ("0038_form_permissions.sql", SchemaProbe::Relation("form_permissions")),
    (
        "0039_universal_forms_pivot_view_type.sql",
        SchemaProbe::ConstraintContains("form_views", "form_views_type_check", "pivot"),
    ),
    (
        "0040_universal_forms_gantt_view_type.sql",
        SchemaProbe::ConstraintContains("form_views", "form_views_type_check", "gantt"),
    ),
    (
        "0041_form_record_comments.sql",
        SchemaProbe::Relation("form_record_comments"),
    ),
    (
        "0042_form_attachment_thumbnail_url.sql",
        SchemaProbe::Column("form_attachments", "thumbnail_url"),
    ),
    (
        "0043_form_import_mapping_templates.sql",
        SchemaProbe::Relation("form_import_mapping_templates"),
    ),
    ("0044_form_import_jobs.sql", SchemaProbe::Relation("form_import_jobs")),
    ("0045_form_export_jobs.sql", SchemaProbe::Relation("form_export_jobs")),
    (
        "0046_form_attachment_package_jobs.sql",
        SchemaProbe::Relation("form_attachment_package_jobs"),
    ),
    (
        "0047_form_attachment_media_metadata.sql",
        SchemaProbe::Column("form_attachments", "media_metadata"),
    ),
    ("0048_delivery_lease_and_pickup_indexes.sql", SchemaProbe::Superseded),
    ("0049_agent_invocation_duplicate_tagging.sql", SchemaProbe::Superseded),
    (
        "0050_proposal_workspace_scope.sql",
        SchemaProbe::Column("proposals", "workspace_id"),
    ),
    (
        "0051_bot_operation_logs.sql",
        SchemaProbe::Relation("bot_operation_logs"),
    ),
    (
        "0052_drop_connectors_and_agent_invocations.sql",
        SchemaProbe::RelationAbsent("connectors"),
    ),
    (
        "0053_drop_event_outbox.sql",
        SchemaProbe::RelationAbsent("event_outbox"),
    ),
    ("0054_flow_data_layer.sql", SchemaProbe::Relation("flow_objects")),
    ("0055_flow_import_jobs.sql", SchemaProbe::Relation("flow_import_jobs")),
    // Not `Relation`: this migration creates no table. It adds a generated column, a view and two
    // constraints, and the one object whose *presence* means the invariant is really enforced is
    // the foreign key -- probed by its definition, so a database carrying an older, differently
    // shaped `flow_objects_parent_project_fk` is not mistaken for one that has this migration.
    (
        "0056_flow_objects_parent_project_invariant.sql",
        SchemaProbe::ConstraintContains("flow_objects", "flow_objects_parent_project_fk", "project_scope_id"),
    ),
    // `Relation`, not `ConstraintContains`: this migration creates exactly one object and it is an
    // index, which `to_regclass` resolves the same way it resolves a table or a view. A
    // `ConstraintContains` probe would be wrong here in the way that matters most -- it would
    // report "present" for a database that has `0056`'s constraint but not this index, and a probe
    // that wrongly reports present skips its migration forever.
    (
        "0057_flow_objects_parent_project_scope_index.sql",
        SchemaProbe::Relation("idx_flow_objects_parent_project_scope"),
    ),
    ("0058_flow_search_index.sql", SchemaProbe::Relation("flow_search_index")),
    (
        "0059_flow_navigator_root.sql",
        SchemaProbe::ConstraintAndTrigger(
            "flow_objects",
            "flow_objects_non_navigator_parent_check",
            "parent_id",
            "workspaces_create_flow_navigator_root",
        ),
    ),
    (
        "0060_flow_collections_core.sql",
        SchemaProbe::Relation("flow_collections_core_schema_guard"),
    ),
    (
        "0061_flow_collections_query_indexes.sql",
        SchemaProbe::Relation("flow_collections_query_schema_guard"),
    ),
    (
        "0062_flow_forms_bridge.sql",
        SchemaProbe::Relation("flow_forms_bridge_schema_guard"),
    ),
    (
        "0063_flow_v08_hardening.sql",
        SchemaProbe::Relation("flow_operation_runs"),
    ),
    (
        "0064_flow_v08_replay_requests.sql",
        SchemaProbe::Relation("flow_replay_requests"),
    ),
    (
        "0065_flow_v08_operation_idempotency.sql",
        SchemaProbe::Relation("idx_flow_operation_runs_idempotency"),
    ),
    (
        "0066_flow_v08_object_retention.sql",
        SchemaProbe::Relation("idx_flow_objects_permanent_cleanup"),
    ),
    (
        "0067_flow_package_jobs.sql",
        SchemaProbe::Relation("flow_package_artifacts"),
    ),
    (
        "0068_flow_authz_revocations.sql",
        SchemaProbe::Relation("flow_authz_revocations"),
    ),
    (
        "0069_flow_v08_rollback_control.sql",
        SchemaProbe::Relation("flow_v08_rollback_control"),
    ),
    (
        "0070_bot_operation_logs_surface_check.sql",
        SchemaProbe::ConstraintContains(
            "bot_operation_logs",
            "bot_operation_logs_surface_check",
            "cli_tools_call",
        ),
    ),
    (
        "0071_flow_conversion_job_error_reason.sql",
        SchemaProbe::Column("flow_conversion_jobs", "error_reason"),
    ),
    (
        "0072_flow_conversion_job_started_at.sql",
        SchemaProbe::Column("flow_conversion_jobs", "started_at"),
    ),
    (
        "0073_user_session_version.sql",
        SchemaProbe::Column("users", "session_version"),
    ),
    ("0074_flow_comments.sql", SchemaProbe::Relation("flow_comments")),
    ("0075_flow_share_links.sql", SchemaProbe::Relation("flow_share_links")),
    (
        "0076_flow_page_appearance.sql",
        SchemaProbe::Relation("flow_appearance_assets"),
    ),
    (
        "0077_flow_public_share_rate.sql",
        SchemaProbe::Relation("flow_public_share_hits"),
    ),
    ("0078_flow_scope_expansion.sql", SchemaProbe::Relation("flow_scopes")),
    (
        "0079_flow_scope_constraint_cutover.sql",
        SchemaProbe::Relation("flow_mounts"),
    ),
    (
        "0080_flow_scope_secondary_ownership.sql",
        SchemaProbe::Column("flow_export_jobs", "scope_id"),
    ),
    (
        "0081_flow_scope_typed_ownership.sql",
        SchemaProbe::Column("flow_collection_projections", "scope_id"),
    ),
    (
        "0082_flow_scope_relations_mount_ancestry.sql",
        SchemaProbe::Column("flow_relations", "scope_id"),
    ),
    (
        "0083_flow_export_authority_snapshot.sql",
        SchemaProbe::Column("flow_export_jobs", "source_authz_revision"),
    ),
    (
        "0084_flow_legacy_export_authority.sql",
        SchemaProbe::Column("flow_export_jobs", "artifact_resource_ids"),
    ),
    (
        "0085_flow_ticket_authorization_notifications.sql",
        SchemaProbe::Column("collab_tickets", "authorization_notifications"),
    ),
    (
        "0086_flow_purge_tombstone_receipts.sql",
        SchemaProbe::Column("flow_purge_receipts", "request_body"),
    ),
    (
        "0087_flow_reference_source_scope.sql",
        SchemaProbe::Column("flow_bridge_references", "scope_id"),
    ),
    (
        "0088_flow_scope_import_ownership.sql",
        SchemaProbe::Column("flow_import_jobs", "scope_id"),
    ),
    (
        "0089_flow_export_scope_selection.sql",
        SchemaProbe::Column("flow_export_jobs", "warnings"),
    ),
    (
        "0090_flow_conversion_source_scope.sql",
        SchemaProbe::Column("flow_conversion_jobs", "scope_id"),
    ),
    (
        "0091_flow_operation_scope_ownership.sql",
        SchemaProbe::Column("flow_operation_runs", "owner_flow_scope_id"),
    ),
    (
        "0092_ai_participant_config_version.sql",
        SchemaProbe::Column("ai_participants", "config_updated_at"),
    ),
    (
        "0093_flow_page_body_assets.sql",
        SchemaProbe::ConstraintContains(
            "flow_page_asset_refs",
            "flow_page_asset_refs_scope_id_object_id_asset_id_fkey",
            "FOREIGN KEY (scope_id, object_id, asset_id)",
        ),
    ),
    (
        "0094_flow_child_page_reservations.sql",
        SchemaProbe::Relation("flow_page_child_refs"),
    ),
    ("0095_flow_logical_blocks.sql", SchemaProbe::BlockSharing),
    ("0096_flow_page_block_transfers.sql", SchemaProbe::PageBlockTransfers),
    (
        "0097_flow_portable_page_packages.sql",
        SchemaProbe::Column("flow_runtime_capabilities", "portable_page_packages_enabled"),
    ),
    (
        "0098_flow_portable_graph_packages.sql",
        SchemaProbe::Column("flow_runtime_capabilities", "portable_graph_packages_enabled"),
    ),
    (
        "0099_form_plugin_packages.sql",
        SchemaProbe::Relation("form_plugin_packages"),
    ),
    (
        "0100_form_plugin_layouts.sql",
        SchemaProbe::RelationAbsent("form_plugin_layouts"),
    ),
    (
        "0101_form_plugin_library.sql",
        SchemaProbe::Relation("form_plugin_library"),
    ),
    (
        "0102_flow_integrity_cross_workspace_attempts.sql",
        SchemaProbe::CrossWorkspaceIntegrity,
    ),
    (
        "0103_form_plugin_extensions.sql",
        SchemaProbe::Relation("form_plugin_user_preferences"),
    ),
    (
        "0104_project_resource_kinds.sql",
        SchemaProbe::ConstraintContains(
            "project_resources",
            "project_resources_kind_check",
            "ARRAY['directory'::text, 'custom'::text]",
        ),
    ),
    ("0105_token_bot_entity_type.sql", SchemaProbe::Rerunnable),
    (
        "0106_flow_capabilities_default_on.sql",
        SchemaProbe::Relation("flow_runtime_capability_rollouts"),
    ),
    (
        "0107_flow_block_registration_progress.sql",
        SchemaProbe::Column("flow_block_checkpoints", "pending_registrations"),
    ),
    (
        "0108_flow_block_checkpoint_digest.sql",
        SchemaProbe::Column("flow_block_checkpoints", "registry_digest"),
    ),
    (
        "0109_oauth_authorization_server.sql",
        SchemaProbe::Relation("oauth_grants"),
    ),
    (
        "0110_runner_task_protocol.sql",
        SchemaProbe::Column("ai_tasks", "runner_phase"),
    ),
    ("0111_alert_chain.sql", SchemaProbe::Relation("inbound_events")),
    (
        "0112_bot_connections.sql",
        SchemaProbe::Relation("project_bot_connections"),
    ),
    (
        "0113_ai_task_chain.sql",
        SchemaProbe::Column("ai_tasks", "chain_root_id"),
    ),
    (
        "0114_multi_bot_connections.sql",
        SchemaProbe::Column("project_bot_connections", "channel"),
    ),
    (
        "0115_flow_default_on.sql",
        SchemaProbe::Trigger("workspaces", "flow_provision_workspace_settings"),
    ),
    (
        "0116_webhook_delivery_duration.sql",
        SchemaProbe::Column("webhook_deliveries", "duration_ms"),
    ),
    (
        "0117_sprint_rules.sql",
        SchemaProbe::Relation("sprints_one_active_per_project"),
    ),
    (
        "0118_label_rules.sql",
        SchemaProbe::Relation("labels_workspace_lower_name_key"),
    ),
    (
        "0119_plugin_single_active_version.sql",
        SchemaProbe::Relation("plugins_one_active_version"),
    ),
    ("0120_email_outbox.sql", SchemaProbe::Column("users", "last_digest_at")),
    (
        "0121_milestones.sql",
        SchemaProbe::Relation("milestones_workspace_lower_name"),
    ),
    (
        "0122_bot_project_access.sql",
        SchemaProbe::Relation("bot_project_grants"),
    ),
    (
        "0123_work_item_page_links.sql",
        SchemaProbe::Relation("work_item_page_links_object"),
    ),
    ("0124_comment_mentions.sql", SchemaProbe::Column("comments", "mentions")),
    ("0125_oauth_client_scopes.sql", SchemaProbe::Rerunnable),
    (
        "0126_retire_unused_columns.sql",
        SchemaProbe::ColumnAbsent("users", "agent_config"),
    ),
    ("0127_retire_legacy_objects.sql", SchemaProbe::Rerunnable),
    (
        "0130_tmux_cli_codex.sql",
        SchemaProbe::ConstraintContains("project_bot_connections", "pbc_tmux_cli_check", "'codex'"),
    ),
    (
        "0131_drop_one_tmux_per_cli.sql",
        SchemaProbe::RelationAbsent("pbc_one_tmux_per_cli"),
    ),
    (
        "0132_change_hints.sql",
        SchemaProbe::Trigger("users", "trg_sv_hint_bot_user_active"),
    ),
    ("0133_capability_ids.sql", SchemaProbe::Relation("capability_ids")),
    ("0134_packages.sql", SchemaProbe::Relation("packages")),
    ("0135_layouts.sql", SchemaProbe::Relation("layout_placements")),
    ("0136_form_schema_v2.sql", SchemaProbe::Rerunnable),
    (
        "0137_sites.sql",
        SchemaProbe::Trigger("form_records", "trg_sv_site_record_guard"),
    ),
    ("0138_installations.sql", SchemaProbe::Relation("extension_invocations")),
    (
        "0139_artefact_drafts.sql",
        SchemaProbe::Trigger("artefact_drafts", "trg_sv_hint_draft_status"),
    ),
    ("0140_nav.sql", SchemaProbe::Relation("nav_user_prefs")),
    ("0142_cap_record_titles.sql", SchemaProbe::Rerunnable),
];

/// One recorded migration outcome.
struct RecordedMigration {
    checksum: String,
    status: String,
}

/// What the runner did with one migration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MigrationOutcome {
    /// The ledger already records a success.
    Skipped,
    /// Recognised in a database that predates the ledger and recorded without executing it.
    Adopted,
    /// Executed and recorded in the same transaction.
    Applied,
}

/// Recovery switches for one migration run.
///
/// Kept as a value rather than read from the environment deep inside the runner, so both branches
/// can be exercised by a test without mutating process wide state.
#[derive(Clone, Copy, Default)]
pub struct MigrationOptions {
    /// Re-execute every migration, including those the ledger already records as successful.
    replay: bool,
    /// Start even though a migration failed or the post-run schema check found a gap.
    continue_on_error: bool,
}

impl MigrationOptions {
    /// Maps the `[migrations]` section onto the runner's switches.
    ///
    /// A separate function so the mapping is testable without a database: the two switches have
    /// the same type and opposite consequences, and swapping them would otherwise only show up
    /// on a deployment that had asked for one of them.
    const fn from_config(migrations: MigrationsConfig) -> Self {
        Self {
            replay: migrations.replay,
            continue_on_error: migrations.continue_on_error,
        }
    }
}

pub async fn run_migrations(db: &DatabaseConnection, migrations: MigrationsConfig) -> anyhow::Result<()> {
    run_migrations_with(db, MigrationOptions::from_config(migrations)).await
}

pub async fn run_migrations_with(db: &DatabaseConnection, options: MigrationOptions) -> anyhow::Result<()> {
    let Some((ledger_name, ledger_sql)) = MIGRATIONS.first() else {
        return Err(anyhow::anyhow!("migration list is empty, refusing to start"));
    };

    // The ledger bootstrap is idempotent by construction, so it is the one file that may run on
    // every start: nothing can be read from or written to the ledger before it exists. It still
    // takes the runner lock, because two replicas issuing `CREATE TABLE IF NOT EXISTS` at the
    // same instant can collide on the catalogue instead of one of them seeing the finished table.
    bootstrap_migration_ledger(db, ledger_name, ledger_sql)
        .await
        .map_err(|err| anyhow::anyhow!("migration ledger bootstrap failed: {err}"))?;
    record_migration_if_absent(db, ledger_name, &migration_checksum(ledger_sql)).await?;

    let MigrationOptions {
        replay,
        continue_on_error,
    } = options;
    if replay {
        tracing::warn!(
            setting = MIGRATION_REPLAY_KEY,
            "migration replay requested: every migration is re-executed, failures are reported \
             but not fatal, and a ledger row that already records a success is never downgraded"
        );
    }
    if continue_on_error {
        tracing::warn!(
            setting = MIGRATION_CONTINUE_ON_ERROR_KEY,
            "starting even if a migration fails or the schema check finds a gap; the failures are \
             recorded in schema_migrations and retried on the next start"
        );
    }
    let tolerate_failure = replay || continue_on_error;

    let mut adopted = 0_usize;
    let mut applied = 0_usize;
    let mut failed: Vec<&str> = Vec::new();

    for (name, sql) in MIGRATIONS.iter().skip(1) {
        match run_one_migration(db, name, sql, replay).await {
            Ok(MigrationOutcome::Skipped) => {}
            Ok(MigrationOutcome::Adopted) => {
                adopted += 1;
                tracing::info!(migration = %name, "migration recognised in an existing database and adopted");
            }
            Ok(MigrationOutcome::Applied) => {
                applied += 1;
                tracing::info!(migration = %name, "migration applied");
            }
            Err(err) => {
                failed.push(name);
                tracing::error!(migration = %name, error = %err, "migration failed");
                if !tolerate_failure {
                    return Err(anyhow::anyhow!("migration {name} failed: {err}"));
                }
            }
        }
    }

    if adopted > 0 {
        tracing::warn!(
            adopted,
            "migrations were adopted from an existing database without executing them; the schema \
             check below decides whether that database is actually complete"
        );
    }
    tracing::info!(applied, adopted, failed = failed.len(), "migration run finished");

    verify_recorded_migrations(db, tolerate_failure).await
}

/// Runs one migration under the runner lock, deciding inside the transaction that will also write
/// the ledger row. Decision, execution and bookkeeping are therefore one atomic step: an
/// interrupted run leaves no half claimed ledger, and a second replica re-reads the ledger under
/// the same lock instead of acting on a snapshot taken before it was blocked.
async fn run_one_migration(
    db: &DatabaseConnection,
    name: &str,
    sql: &str,
    replay: bool,
) -> anyhow::Result<MigrationOutcome> {
    let checksum = migration_checksum(sql);
    let txn = db.begin().await?;
    lock_migration_runner(&txn).await?;

    let recorded = load_recorded_migration(&txn, name).await?;
    let succeeded_before = recorded.as_ref().is_some_and(|row| row.status != "failed");

    if !replay && let Some(row) = &recorded {
        if succeeded_before {
            if row.checksum != checksum {
                tracing::warn!(
                    migration = %name,
                    recorded_status = %row.status,
                    "migration file changed after it was recorded; the database still holds the \
                     previously applied version"
                );
            }
            txn.commit().await?;
            return Ok(MigrationOutcome::Skipped);
        }
        tracing::warn!(
            migration = %name,
            "migration is recorded as failed; retrying it"
        );
    }

    if !replay && recorded.is_none() && name <= MIGRATION_ADOPTION_CUTOFF && migration_is_present(&txn, name).await? {
        record_migration_with_conn(&txn, name, &checksum, "adopted", None).await?;
        txn.commit().await?;
        return Ok(MigrationOutcome::Adopted);
    }

    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        let message = err.to_string();
        // A replay re-executes files that already ran, so "relation already exists" is the
        // expected answer there. Downgrading the ledger row to `failed` for those was what turned
        // a single use of the replay switch into a database that could never start normally
        // again, because an ordinary start retries `failed` rows and fails on the same statement.
        if succeeded_before {
            tracing::warn!(
                migration = %name,
                error = %message,
                "re-execution failed; the ledger keeps the recorded success"
            );
        } else if let Err(record_err) = record_migration(db, name, &checksum, "failed", Some(&message)).await {
            tracing::error!(
                migration = %name,
                error = %record_err,
                "could not record the migration failure in the ledger"
            );
        }
        return Err(anyhow::anyhow!("{message}"));
    }

    record_migration_with_conn(&txn, name, &checksum, "applied", None).await?;
    txn.commit().await?;
    Ok(MigrationOutcome::Applied)
}

/// Creates the ledger table itself, under the runner lock so concurrent starts serialise.
async fn bootstrap_migration_ledger(db: &DatabaseConnection, name: &str, sql: &str) -> anyhow::Result<()> {
    let txn = db.begin().await?;
    lock_migration_runner(&txn).await?;
    if let Err(err) = txn.execute_unprepared(sql).await {
        if let Err(rollback_err) = txn.rollback().await {
            tracing::warn!(migration = %name, error = %rollback_err, "migration rollback failed");
        }
        return Err(err.into());
    }
    txn.commit().await?;
    Ok(())
}

/// Takes the transaction scoped runner lock. It is released by the commit or the rollback that
/// ends the transaction, so it can never be left behind on a pooled connection.
async fn lock_migration_runner<C: ConnectionTrait>(conn: &C) -> anyhow::Result<()> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1, $2)",
        vec![MIGRATION_LOCK_NAMESPACE.into(), MIGRATION_LOCK_ID.into()],
    ))
    .await?;
    Ok(())
}

/// Content fingerprint used to notice that a migration file changed after it was applied.
fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

fn migration_probe(name: &str) -> Option<SchemaProbe> {
    MIGRATION_PROBES
        .iter()
        .find(|(probe_name, _)| *probe_name == name)
        .map(|(_, probe)| *probe)
}

/// Whether the objects a migration creates are already in the database.
///
/// `Rerunnable` migrations and migrations without a probe answer "no", which costs one execution
/// and never skips work.
async fn migration_is_present<C: ConnectionTrait>(conn: &C, name: &str) -> anyhow::Result<bool> {
    let Some(statement) = migration_probe(name).and_then(SchemaProbe::statement) else {
        return Ok(false);
    };
    let Some(row) = conn.query_one(statement).await? else {
        return Ok(false);
    };
    Ok(row.try_get::<bool>("", "present")?)
}

/// Confirms that every migration the ledger reports as successful really left its objects behind.
///
/// This is the assertion the previous revision did not have. `verify_governance_schema` only
/// covers governance and `verify_delivery_schema` only covers the two delivery migrations, so a
/// database that claimed 0001..0047 while the forms tables were missing started up healthy and
/// answered 500 for every forms request. Every migration is checked here instead.
async fn verify_recorded_migrations(db: &DatabaseConnection, tolerate_failure: bool) -> anyhow::Result<()> {
    let ledger = load_migration_ledger(db).await?;
    let mut missing: Vec<&str> = Vec::new();

    for (name, _) in MIGRATIONS {
        let Some(recorded) = ledger.get(*name) else {
            continue;
        };
        if recorded.status == "failed" {
            continue;
        }
        if migration_probe(name).is_some_and(|probe| matches!(probe, SchemaProbe::Rerunnable | SchemaProbe::Superseded))
        {
            continue;
        }
        if !migration_is_present(db, name).await? {
            missing.push(name);
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let names = missing.join(", ");
    if tolerate_failure {
        tracing::error!(
            migrations = %names,
            "the ledger reports these migrations as applied but their objects are missing; \
             starting anyway because a migration escape hatch is set"
        );
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "the migration ledger reports {names} as applied but their objects are missing; re-run \
         with {MIGRATION_REPLAY_KEY} = true in the configuration file to re-execute every \
         migration, or start with {MIGRATION_CONTINUE_ON_ERROR_KEY} = true to inspect the database"
    ))
}

async fn record_migration(
    db: &DatabaseConnection,
    name: &str,
    checksum: &str,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    record_migration_with_conn(db, name, checksum, status, error).await
}

async fn record_migration_with_conn<C: ConnectionTrait>(
    conn: &C,
    name: &str,
    checksum: &str,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO schema_migrations (name, checksum, status, error, applied_at)
            VALUES ($1, $2, $3, $4, now())
            ON CONFLICT (name) DO UPDATE
            SET checksum = EXCLUDED.checksum,
                status = EXCLUDED.status,
                error = EXCLUDED.error,
                applied_at = EXCLUDED.applied_at
        ",
        vec![
            name.into(),
            checksum.into(),
            status.into(),
            error.map(ToString::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Records the ledger bootstrap without touching an existing row, so `applied_at` keeps meaning
/// "when this database first got the migration" instead of "when the API last started".
async fn record_migration_if_absent(db: &DatabaseConnection, name: &str, checksum: &str) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO schema_migrations (name, checksum, status, error, applied_at)
            VALUES ($1, $2, 'applied', NULL, now())
            ON CONFLICT (name) DO NOTHING
        ",
        vec![name.into(), checksum.into()],
    ))
    .await?;
    Ok(())
}

async fn load_recorded_migration<C: ConnectionTrait>(
    conn: &C,
    name: &str,
) -> anyhow::Result<Option<RecordedMigration>> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT checksum, status FROM schema_migrations WHERE name = $1",
            vec![name.into()],
        ))
        .await?;
    match row {
        Some(row) => Ok(Some(RecordedMigration {
            checksum: row.try_get::<String>("", "checksum")?,
            status: row.try_get::<String>("", "status")?,
        })),
        None => Ok(None),
    }
}

async fn load_migration_ledger(db: &DatabaseConnection) -> anyhow::Result<HashMap<String, RecordedMigration>> {
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT name, checksum, status FROM schema_migrations".to_string(),
        ))
        .await?;

    let mut ledger = HashMap::with_capacity(rows.len());
    for row in rows {
        let name = row.try_get::<String>("", "name")?;
        let checksum = row.try_get::<String>("", "checksum")?;
        let status = row.try_get::<String>("", "status")?;
        ledger.insert(name, RecordedMigration { checksum, status });
    }
    Ok(ledger)
}

pub async fn verify_governance_schema(db: &DatabaseConnection) -> anyhow::Result<()> {
    const CHECKS: &[(&str, &str)] = &[
        ("proposals table", "SELECT 1 FROM proposals LIMIT 1"),
        ("proposal_type enum", "SELECT 'feature'::proposal_type"),
        ("proposal_status enum", "SELECT 'draft'::proposal_status"),
        ("author_type enum", "SELECT 'human'::author_type"),
        ("trust_scores table", "SELECT 1 FROM trust_scores LIMIT 1"),
        ("trust_score_logs table", "SELECT 1 FROM trust_score_logs LIMIT 1"),
        ("ai_participants table", "SELECT 1 FROM ai_participants LIMIT 1"),
        ("decision_domains table", "SELECT 1 FROM decision_domains LIMIT 1"),
        ("veto_events table", "SELECT 1 FROM veto_events LIMIT 1"),
        ("appeals table", "SELECT 1 FROM appeals LIMIT 1"),
        ("impact_reviews table", "SELECT 1 FROM impact_reviews LIMIT 1"),
        ("review_participants table", "SELECT 1 FROM review_participants LIMIT 1"),
        ("ai_learning_records table", "SELECT 1 FROM ai_learning_records LIMIT 1"),
        (
            "decision_audit_reports table",
            "SELECT 1 FROM decision_audit_reports LIMIT 1",
        ),
        ("feedback_loop_links table", "SELECT 1 FROM feedback_loop_links LIMIT 1"),
        ("governance_configs table", "SELECT 1 FROM governance_configs LIMIT 1"),
        (
            "governance_audit_logs table",
            "SELECT 1 FROM governance_audit_logs LIMIT 1",
        ),
        ("check_results table", "SELECT 1 FROM check_results LIMIT 1"),
        ("trust_level enum", "SELECT 'observer'::trust_level"),
        ("participant_type enum", "SELECT 'human'::participant_type"),
        ("veto_status enum", "SELECT 'active'::veto_status"),
        ("appeal_status enum", "SELECT 'pending'::appeal_status"),
        ("review_status enum", "SELECT 'pending'::review_status"),
        ("review_rating enum", "SELECT 'A'::review_rating"),
    ];

    for (name, sql) in CHECKS {
        if let Err(e) = db
            .query_one(Statement::from_string(DbBackend::Postgres, (*sql).to_string()))
            .await
        {
            return Err(anyhow::anyhow!("governance schema check failed for {name}: {e}"));
        }
    }

    Ok(())
}

#[cfg(test)]
// Test modules index fixtures directly, print skip reasons, and mirror wire shapes;
// panicking there is the assertion rather than a defect.
#[allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::doc_markdown,
    clippy::redundant_pub_crate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons
)]
mod tests;

/// Migration runner behaviour against a real PostgreSQL server.
///
/// Adoption, replay and retry only differ on databases that are not empty, so a fresh compose
/// database exercises none of them. Each test here creates its own database, shapes it into the
/// state under test and runs the real runner against it.
///
/// Set `OPENPR_TEST_DATABASE_URL` to a maintenance connection string, for example
/// `postgres://user:pw@127.0.0.1:5432/postgres`. Without it these tests report that they were
/// skipped instead of pretending to pass.
#[cfg(test)]
// Test modules index fixtures directly, print skip reasons, and mirror wire shapes;
// panicking there is the assertion rather than a defect.
#[allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::doc_markdown,
    clippy::redundant_pub_crate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons
)]
pub(crate) mod db_tests;
