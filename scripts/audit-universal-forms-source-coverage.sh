#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_TOOL_COUNT="$(python3 "$ROOT_DIR/skills/sylvode-mcp/scripts/expected-tool-count.py")"

usage() {
  cat <<'EOF'
Usage: scripts/audit-universal-forms-source-coverage.sh

Audits that the universal forms surface still has concrete source-code,
migration, frontend, MCP, worker, and smoke-test entrypoints. This is a fast
structural gate; behavior is covered by the smoke scripts.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

failures=0

check_file() {
  local path="$1"
  if [[ -f "$ROOT_DIR/$path" ]]; then
    printf 'PASS: file exists: %s\n' "$path"
  else
    printf 'FAIL: file missing: %s\n' "$path" >&2
    failures=$((failures + 1))
  fi
}

check_executable() {
  local path="$1"
  if [[ -x "$ROOT_DIR/$path" ]]; then
    printf 'PASS: executable file: %s\n' "$path"
  else
    printf 'FAIL: file not executable: %s\n' "$path" >&2
    failures=$((failures + 1))
  fi
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$ROOT_DIR/$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing in %s: %s\n' "$path" "$needle" >&2
    failures=$((failures + 1))
  fi
}

contains_regex() {
  local description="$1"
  local path="$2"
  local pattern="$3"
  if rg -q -- "$pattern" "$ROOT_DIR/$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing pattern in %s: %s\n' "$path" "$pattern" >&2
    failures=$((failures + 1))
  fi
}

not_contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  # A path that does not exist contains nothing, so without this a check aimed at a moved
  # or renamed file would pass forever.
  if [[ ! -e "$ROOT_DIR/$path" ]]; then
    printf 'FAIL: %s\n' "$description" >&2
    printf '  checked file does not exist: %s\n' "$path" >&2
    failures=$((failures + 1))
    return
  fi
  if rg -q --fixed-strings -- "$needle" "$ROOT_DIR/$path"; then
    printf 'FAIL: %s\n' "$description" >&2
    printf '  forbidden in %s: %s\n' "$path" "$needle" >&2
    failures=$((failures + 1))
  else
    printf 'PASS: %s\n' "$description"
  fi
}

workflow_step_contains() {
  local description="$1"
  local path="$2"
  local step_name="$3"
  local needle="$4"
  if awk -v step="      - name: ${step_name}" -v needle="$needle" '
    $0 == step { in_block = 1 }
    in_block && index($0, needle) { found = 1 }
    in_block && $0 ~ /^      - name: / && $0 != step { exit found ? 0 : 1 }
    END { exit found ? 0 : 1 }
  ' "$ROOT_DIR/$path"; then
    printf 'PASS: %s\n' "$description"
  else
    printf 'FAIL: %s\n' "$description" >&2
    printf '  missing in %s step %s: %s\n' "$path" "$step_name" "$needle" >&2
    failures=$((failures + 1))
  fi
}

printf 'Universal forms source coverage audit\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

printf 'CI gate coverage:\n'
check_file ".github/workflows/ci.yml"
check_file "scripts/ci-universal-forms-gates.sh"
check_executable "scripts/ci-universal-forms-gates.sh"
contains "CI defines universal forms gate job" ".github/workflows/ci.yml" "universal-forms:"
contains "CI installs cargo machete" ".github/workflows/ci.yml" "cargo install cargo-machete --locked"
contains "CI clears warning-as-error boundary for cargo-machete install" ".github/workflows/ci.yml" 'RUSTFLAGS: ""'
workflow_step_contains "CI clears RUSTFLAGS inside cargo-machete install step" ".github/workflows/ci.yml" "Unused deps" 'RUSTFLAGS: ""'
contains "CI clears warning-as-error boundary for cargo-audit install" ".github/workflows/ci.yml" 'RUSTFLAGS: ""'
workflow_step_contains "CI clears RUSTFLAGS inside cargo-audit install step" ".github/workflows/ci.yml" "Install audit tools" 'RUSTFLAGS: ""'
contains "CI uses universal forms gate wrapper" ".github/workflows/ci.yml" "bash scripts/ci-universal-forms-gates.sh"
contains "CI wrapper runs PostgreSQL-only security scope audit" "scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-security-scope.sh"
contains "CI wrapper runs universal forms source coverage audit" "scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-source-coverage.sh"
contains "CI wrapper runs universal forms production readiness audit" "scripts/ci-universal-forms-gates.sh" "scripts/audit-universal-forms-production-readiness.sh"

printf 'Migration and backend source files:\n'
for path in \
  migrations/0030_universal_forms.sql \
  migrations/0031_business_events_outbox_inbox.sql \
  migrations/0032_print_device_connector_kinds.sql \
  migrations/0033_plugins_wasm.sql \
  migrations/0034_user_profile_preferences.sql \
  migrations/0038_form_permissions.sql \
  apps/api/src/forms/mod.rs \
  apps/api/src/forms/schema.rs \
  apps/api/src/forms/values.rs \
  apps/api/src/forms/decimal.rs \
  apps/api/src/forms/validation.rs \
  apps/api/src/forms/projections.rs \
  apps/api/src/plugins/manifest.rs \
  apps/api/src/plugins/runtime.rs \
  apps/api/src/plugins/hooks.rs \
  apps/api/src/events/mod.rs \
  apps/api/src/routes/form.rs \
  apps/api/src/routes/plugin.rs \
  apps/api/src/routes/webhook.rs \
  apps/api/src/services/webhooks.rs \
  apps/api/src/routes/check_result.rs \
  apps/api/src/routes/project_type.rs \
  apps/worker/src/main.rs; do
  check_file "$path"
done

printf '\nDatabase contract coverage:\n'
contains "universal forms migration creates project_forms" "migrations/0030_universal_forms.sql" "CREATE TABLE IF NOT EXISTS project_forms"
contains "universal forms migration creates form_records" "migrations/0030_universal_forms.sql" "CREATE TABLE IF NOT EXISTS form_records"
contains "universal forms migration creates form_record_links" "migrations/0030_universal_forms.sql" "CREATE TABLE IF NOT EXISTS form_record_links"
contains "universal forms migration creates decimal projection index" "migrations/0030_universal_forms.sql" "value_decimal"
contains "event migration creates business_events" "migrations/0031_business_events_outbox_inbox.sql" "CREATE TABLE IF NOT EXISTS business_events"
contains "event migration creates event_outbox" "migrations/0031_business_events_outbox_inbox.sql" "CREATE TABLE IF NOT EXISTS event_outbox"
contains "event migration creates event_inbox" "migrations/0031_business_events_outbox_inbox.sql" "CREATE TABLE IF NOT EXISTS event_inbox"
contains "print/device migration includes print connector kind" "migrations/0032_print_device_connector_kinds.sql" "print"
contains "print/device migration includes device connector kind" "migrations/0032_print_device_connector_kinds.sql" "device"
contains "plugin migration creates plugins table" "migrations/0033_plugins_wasm.sql" "CREATE TABLE IF NOT EXISTS plugins"
contains "plugin migration creates plugin invocations table" "migrations/0033_plugins_wasm.sql" "CREATE TABLE IF NOT EXISTS plugin_invocations"
contains "user profile migration adds avatar URL" "migrations/0034_user_profile_preferences.sql" "avatar_url"
contains "user profile migration adds notification preferences" "migrations/0034_user_profile_preferences.sql" "notification_prefs"
contains "form permission migration creates form_permissions" "migrations/0038_form_permissions.sql" "CREATE TABLE IF NOT EXISTS form_permissions"
contains "form permission migration supports role subjects" "migrations/0038_form_permissions.sql" "subject_type IN ('role')"
contains "API build generates the migration registry from migrations/" "apps/api/build.rs" "write_migration_registry"

printf '\nBackend behavior coverage:\n'
contains "decimal normalization uses rust_decimal" "apps/api/src/forms/decimal.rs" "rust_decimal"
contains "record values reject amount JSON number" "apps/api/src/forms/types/numeric.rs" "if raw.is_number()"
contains "forms projection writes decimal index" "apps/api/src/forms/projections.rs" "value_decimal"
contains "form route writes business events" "apps/api/src/routes/form.rs" "business_events"
contains "business event helper writes the event ledger" "apps/api/src/events/mod.rs" "INSERT INTO business_events"
contains "form create path emits form created events" "apps/api/src/forms/native_create.rs" "\"form.created\""
contains "form route exposes duplicate handler" "apps/api/src/routes/form.rs" "duplicate_form"
contains "form route emits duplicated events" "apps/api/src/routes/form.rs" "form.duplicated"
contains "form route emits form updated events" "apps/api/src/routes/form.rs" "form.updated"
contains "form route emits form archived events" "apps/api/src/routes/form.rs" "form.archived"
contains "form route emits view created events" "apps/api/src/routes/form.rs" "form.view.created"
contains "form route emits view archived events" "apps/api/src/routes/form.rs" "form.view.archived"
contains "form route handles record links" "apps/api/src/routes/form.rs" "form_record_links"
contains "form route emits print job created" "apps/api/src/routes/form.rs" "print_job.created"
contains "form route exposes import preview handler" "apps/api/src/routes/form.rs" "preview_import_form_records"
contains "form route exposes import commit handler" "apps/api/src/routes/form.rs" "import_form_records"
contains "form import reuses shared record creation" "apps/api/src/routes/form.rs" "create_record_for_form"
contains "form route normalizes write idempotency keys" "apps/api/src/routes/form.rs" "normalize_optional_idempotency_key"
contains "form route resolves existing idempotency receipts" "apps/api/src/routes/form.rs" "find_idempotent_record"
contains "form record create writes idempotency receipts" "apps/api/src/routes/form.rs" "form.record.created"
contains "form event helper forwards idempotency keys" "apps/api/src/routes/form.rs" "insert_form_event_with_idempotency"
contains "API registers form import preview route" "apps/api/src/router/forms.rs" "/api/v1/forms/{form_id}/records/import-preview"
contains "API registers form import commit route" "apps/api/src/router/forms.rs" "/api/v1/forms/{form_id}/records/import"
contains "API registers form duplicate route" "apps/api/src/router/forms.rs" "/api/v1/forms/{form_id}/duplicate"
contains "form route exposes permissions getter" "apps/api/src/routes/form.rs" "get_form_permissions"
contains "form route exposes permissions updater" "apps/api/src/routes/form.rs" "update_form_permissions"
contains "form route enforces role permissions" "apps/api/src/routes/form.rs" "require_form_action"
contains "form route denies permission actions" "apps/api/src/routes/form.rs" "form permission denied"
contains "API registers form permission route" "apps/api/src/router/forms.rs" "/api/v1/forms/{form_id}/permissions"
contains "plugin runtime uses wasmtime" "apps/api/src/plugins/runtime.rs" "wasmtime"
contains "plugin hooks support field validators" "apps/api/src/plugins/hooks.rs" "field_validator"
contains "plugin hooks support formula hooks" "apps/api/src/plugins/hooks.rs" "formula"
contains "plugin hooks support event handlers" "apps/api/src/plugins/hooks.rs" "event_handler"
contains "plugin route emits installed business events" "apps/api/src/routes/plugin.rs" "plugin.installed"
contains "plugin route emits updated business events" "apps/api/src/routes/plugin.rs" "plugin.updated"
contains "plugin route emits invoked business events" "apps/api/src/routes/plugin.rs" "plugin.invoked"
contains "plugin hooks emit invoked business events" "apps/api/src/plugins/hooks.rs" "plugin.invoked"
contains "plugin hooks mark automatic hook metadata" "apps/api/src/plugins/hooks.rs" "automatic_hook"
contains "restaurant order line total is a formula field of the template package" "packages/builtin/restaurant-ordering/contents/forms/order_line.json" "\"op\": \"multiply\""
not_contains "restaurant template installs no calculation plugin" "apps/api/src/routes/project.rs" "restaurant_calc"
contains "project route emits project created business events" "apps/api/src/routes/project.rs" "project.created"
contains "project route emits project updated business events" "apps/api/src/routes/project.rs" "project.updated"
contains "project route emits project deleted business events" "apps/api/src/routes/project.rs" "project.deleted"
contains "project type route emits created business events" "apps/api/src/routes/project_type.rs" "project_type.created"
contains "project type route emits updated business events" "apps/api/src/routes/project_type.rs" "project_type.updated"
contains "project resource route emits created business events" "apps/api/src/routes/project_type.rs" "project_resource.created"
contains "project resource route emits updated business events" "apps/api/src/routes/project_type.rs" "project_resource.updated"
contains "project resource route emits deleted business events" "apps/api/src/routes/project_type.rs" "project_resource.deleted"
contains "auth route updates current user profile" "apps/api/src/routes/auth.rs" "update_profile"
contains "auth route verifies current password before update" "apps/api/src/routes/auth.rs" "invalid current password"
contains "auth route persists notification preferences" "apps/api/src/routes/auth.rs" "update_preferences"

printf '\nWorker and event delivery coverage:\n'
contains "worker emits AI task business events" "apps/worker/src/ai_tasks.rs" "ai_task.picked_up"
contains "webhook service emits created business events" "apps/api/src/services/webhooks.rs" "webhook.created"
contains "webhook service emits updated business events" "apps/api/src/services/webhooks.rs" "webhook.updated"
contains "webhook service emits deleted business events" "apps/api/src/services/webhooks.rs" "webhook.deleted"
contains "check result route emits created business events" "apps/api/src/routes/check_result.rs" "check_result.created"
contains "check result route emits proposed business events" "apps/api/src/routes/check_result.rs" "check_result.proposed"
contains "check result route emits proposal business events" "apps/api/src/routes/check_result.rs" "proposal.created_from_check_result"
contains "API registers form schema version list route" "apps/api/src/router/forms.rs" "/api/v1/forms/{form_id}/schema-versions"
contains "API registers form schema version get route" "apps/api/src/router/forms.rs" "/api/v1/forms/{form_id}/schema-versions/{version}"
contains "API registers child create route" "apps/api/src/router/forms.rs" "post(routes::form::create_child_record)"
contains "API registers child update route" "apps/api/src/router/forms.rs" "patch(routes::form::update_child_record)"
contains "API registers child archive route" "apps/api/src/router/forms.rs" "delete(routes::form::archive_child_record)"
contains "API registers child restore route" "apps/api/src/router/forms.rs" "post(routes::form::restore_child_record)"
contains "form route lists schema versions" "apps/api/src/routes/form.rs" "list_form_schema_versions"
contains "form route gets one schema version" "apps/api/src/routes/form.rs" "get_form_schema_version"
contains "form route creates child records" "apps/api/src/routes/form.rs" "create_child_record"
contains "form route updates child records" "apps/api/src/routes/form.rs" "update_child_record"
contains "form route archives child records" "apps/api/src/routes/form.rs" "archive_child_record"
contains "form route restores child records" "apps/api/src/routes/form.rs" "restore_child_record"
contains "forms MCP smoke calls schema version list" "scripts/smoke-forms-mcp.sh" "form_schema_versions.list"
contains "forms MCP smoke calls schema version get" "scripts/smoke-forms-mcp.sh" "form_schema_versions.get"
contains "forms MCP smoke calls child create" "scripts/smoke-forms-mcp.sh" "form_records.child_create"
contains "forms MCP smoke calls child update" "scripts/smoke-forms-mcp.sh" "form_records.child_update"
contains "forms MCP smoke calls child archive" "scripts/smoke-forms-mcp.sh" "form_records.child_archive"
contains "forms MCP smoke calls child restore" "scripts/smoke-forms-mcp.sh" "form_records.child_restore"
contains "forms MCP smoke verifies record create idempotency" "scripts/smoke-forms-mcp.sh" "MCP create idempotency should return the original record on retry"
contains "forms MCP smoke verifies import idempotency" "scripts/smoke-forms-mcp.sh" "MCP import idempotency should not write duplicate retry rows"

printf '\nMCP and CLI coverage:\n'
for path in \
  apps/mcp-server/src/tools/forms.rs \
  apps/mcp-server/src/tools/plugins.rs \
  apps/mcp-server/src/tools/project_types.rs \
  apps/mcp-server/src/cli.rs; do
  check_file "$path"
done
contains "MCP exposes forms.list" "apps/mcp-server/src/tools/forms.rs" "forms.list"
contains "MCP exposes forms.duplicate" "apps/mcp-server/src/tools/forms.rs" "forms.duplicate"
contains "MCP exposes forms.schema_summary" "apps/mcp-server/src/tools/forms.rs" "forms.schema_summary"
contains "MCP exposes forms.field_usage" "apps/mcp-server/src/tools/forms.rs" "forms.field_usage"
contains "MCP exposes forms.field_dependencies" "apps/mcp-server/src/tools/forms.rs" "forms.field_dependencies"
contains "MCP exposes form schema version list" "apps/mcp-server/src/tools/forms.rs" "form_schema_versions.list"
contains "MCP exposes form schema version get" "apps/mcp-server/src/tools/forms.rs" "form_schema_versions.get"
contains "MCP exposes form_permissions.get" "apps/mcp-server/src/tools/forms.rs" "form_permissions.get"
contains "MCP exposes form_permissions.update" "apps/mcp-server/src/tools/forms.rs" "form_permissions.update"
contains "MCP exposes form_attachments.list" "apps/mcp-server/src/tools/forms.rs" "form_attachments.list"
contains "MCP exposes form_attachments.create" "apps/mcp-server/src/tools/forms.rs" "form_attachments.create"
contains "MCP exposes form_attachments.archive" "apps/mcp-server/src/tools/forms.rs" "form_attachments.archive"
contains "MCP exposes form_attachments.restore" "apps/mcp-server/src/tools/forms.rs" "form_attachments.restore"
contains "MCP exposes form_records.create" "apps/mcp-server/src/tools/forms.rs" "form_records.create"
contains "MCP form write tools expose idempotency keys" "apps/mcp-server/src/tools/forms.rs" "Optional write receipt key for retry-safe automation"
contains "MCP exposes form_records.export" "apps/mcp-server/src/tools/forms.rs" "form_records.export"
contains "MCP exposes form_records.import_preview" "apps/mcp-server/src/tools/forms.rs" "form_records.import_preview"
contains "MCP exposes form_records.import_commit" "apps/mcp-server/src/tools/forms.rs" "form_records.import_commit"
contains "MCP exposes form_records.relation_targets" "apps/mcp-server/src/tools/forms.rs" "form_records.relation_targets"
contains "MCP exposes form_records.children" "apps/mcp-server/src/tools/forms.rs" "form_records.children"
contains "MCP exposes form_records.child_create" "apps/mcp-server/src/tools/forms.rs" "form_records.child_create"
contains "MCP exposes form_records.child_update" "apps/mcp-server/src/tools/forms.rs" "form_records.child_update"
contains "MCP exposes form_records.child_archive" "apps/mcp-server/src/tools/forms.rs" "form_records.child_archive"
contains "MCP exposes form_records.child_restore" "apps/mcp-server/src/tools/forms.rs" "form_records.child_restore"
contains "MCP exposes form_records.aggregate" "apps/mcp-server/src/tools/forms.rs" "form_records.aggregate"
contains "MCP exposes events.tail" "apps/mcp-server/src/tools/forms.rs" "events.tail"
contains "MCP exposes plugin install" "apps/mcp-server/src/tools/plugins.rs" "plugins.install"
contains "MCP exposes plugin invoke" "apps/mcp-server/src/tools/plugins.rs" "plugins.invoke"
contains "MCP registry pins current tool count" "apps/mcp-server/src/tools/mod.rs" "tools.len(),"
contains "MCP registry embeds its surface snapshot" "apps/mcp-server/src/tools/mod.rs" 'include_str!("mcp-surface.snapshot.md")'
contains "MCP skill validation derives the expected registry count" "skills/sylvode-mcp/scripts/validate-mcp.sh" "expected-tool-count.py"
contains "MCP regression derives the expected registry count" "skills/sylvode-mcp/scripts/mcp-regression.py" "expected-tool-count.py"
contains "API agent policy enables form schema version MCP tools" "apps/api/src/routes/context.rs" "form_schema_versions.list"
contains "API agent policy enables form permission MCP tools" "apps/api/src/routes/context.rs" "form_permissions.get"
contains "API agent policy enables form attachment MCP tools" "apps/api/src/routes/context.rs" "form_attachments.create"
contains "API agent policy enables form import/export MCP tools" "apps/api/src/routes/context.rs" "form_records.import_commit"
contains "API agent policy enables relation MCP tools" "apps/api/src/routes/context.rs" "form_records.relation_targets"
contains "API agent policy enables child lifecycle MCP tools" "apps/api/src/routes/context.rs" "form_records.child_archive"
not_contains "MCP guides do not retain stale 82-tool heading" "apps/mcp-server/src/server.rs" "## Tools (82)"
not_contains "MCP guides do not retain stale 65-tool heading" "apps/mcp-server/src/server.rs" "## Tools (65)"
not_contains "MCP validation does not accept stale 65-tool minimum" "skills/sylvode-mcp/scripts/validate-mcp.sh" "-ge 65"
contains "CLI has generic MCP tool caller" "apps/mcp-server/src/cli.rs" "ToolsAction::Call"
contains "CLI examples can call forms.list" "apps/mcp-server/src/cli.rs" "forms.list"

printf '\nFrontend and browser smoke coverage:\n'
for path in \
  frontend/src/lib/api/forms.ts \
  frontend/src/lib/api/auth.ts \
  'frontend/src/routes/(app)/settings/+page.svelte' \
  'frontend/src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte' \
  frontend/scripts/forms-smoke-server.mjs \
  frontend/scripts/smoke-forms-page-interactions.mjs \
  frontend/scripts/smoke-forms-record-list.mjs \
  frontend/scripts/smoke-restaurant-ordering.mjs; do
  check_file "$path"
done
contains "frontend forms API creates records" "frontend/src/lib/api/forms.ts" "createRecord"
contains "frontend forms API updates records" "frontend/src/lib/api/forms.ts" "updateRecord"
contains "frontend forms API aggregates records" "frontend/src/lib/api/forms.ts" "aggregate"
contains "frontend forms API previews record imports" "frontend/src/lib/api/forms.ts" "previewImportRecords"
contains "frontend forms API commits record imports" "frontend/src/lib/api/forms.ts" "importRecords"
contains "frontend forms API gets permissions" "frontend/src/lib/api/forms.ts" "getPermissions"
contains "frontend forms API updates permissions" "frontend/src/lib/api/forms.ts" "updatePermissions"
contains "frontend forms route loads form records" frontend/src/lib/forms/state/loaders.ts "loadRecords"
contains "frontend forms route supports record edit" frontend/src/lib/forms/state/forms-page-state.svelte.ts "editingRecordId"
contains "frontend forms route supports record links" frontend/src/lib/forms/state/record-detail.ts "createRecordLink"
contains "frontend forms route shows print jobs" frontend/src/lib/forms/state/forms-page-state.svelte.ts "print_job"
contains "frontend forms route supports import modal" frontend/src/lib/forms/state/forms-page-state.svelte.ts "showImportRecords"
contains "frontend forms route parses CSV imports" frontend/src/lib/forms/state/import.ts "parseCsvImport"
contains "frontend forms route supports permission panel" frontend/src/lib/forms/state/permissions.ts "saveMemberPermissions"
contains "frontend forms route renders permission actions" frontend/src/lib/forms/page-constants.ts "formPermissionActions"
contains "frontend settings saves profile through API" 'frontend/src/routes/(app)/settings/+page.svelte' "authApi.updateProfile"
contains "frontend settings updates password through API" 'frontend/src/routes/(app)/settings/+page.svelte' "authApi.updatePassword"
contains "frontend settings renders the notification preferences panel" 'frontend/src/routes/(app)/settings/+page.svelte' "<NotificationPreferences />"
contains "frontend notification preferences save through API" frontend/src/lib/notifications/NotificationPreferences.svelte "authApi.updatePreferences"
contains "frontend auth API puts notification preferences" frontend/src/lib/api/auth.ts "/api/v1/auth/me/preferences"
not_contains "frontend notification preferences do not fake save with timeout" frontend/src/lib/notifications/NotificationPreferences.svelte "setTimeout"
not_contains "frontend settings does not fake save with timeout" 'frontend/src/routes/(app)/settings/+page.svelte' "setTimeout"
not_contains "frontend i18n does not expose placeholder settings copy" "frontend/src/lib/i18n/en.json" "placeholder)"
not_contains "frontend zh i18n does not expose placeholder settings copy" "frontend/src/lib/i18n/zh.json" "占位实现"
contains "forms browser smoke covers mobile overflow" "frontend/scripts/smoke-forms-page-interactions.mjs" "horizontal overflow"
contains "forms browser smoke checks overflow at 390 px" "frontend/scripts/smoke-forms-page-interactions.mjs" "document.documentElement.scrollWidth"
contains "forms browser smoke covers the durable record route" "frontend/scripts/smoke-forms-page-interactions.mjs" "the durable record page opens from its own route"
contains "forms browser smoke covers field designer save payload" "frontend/scripts/smoke-forms-page-interactions.mjs" "field design: add a text field with help text and save the schema"
contains "forms browser smoke covers detail layout save payload" "frontend/scripts/smoke-forms-page-interactions.mjs" "detail layout: a section title and description are saved"
contains "forms browser smoke covers permissions save payload" "frontend/scripts/smoke-forms-page-interactions.mjs" "permissions: a role policy is saved with its scope and field denial"
contains "forms browser smoke records its uncovered retired checks" "frontend/scripts/smoke-forms-page-interactions.mjs" "Not covered (the retired smoke asserted them"
contains "forms record list smoke is wired into package scripts" "frontend/package.json" "smoke:forms-record-list"
contains "forms page interactions smoke is wired into package scripts" "frontend/package.json" "smoke:forms-page-interactions"
not_contains "package scripts do not keep the retired forms UI smoke" "frontend/package.json" "smoke:forms-ui"
contains "restaurant browser smoke covers table change" "frontend/scripts/smoke-restaurant-ordering.mjs" "table"
contains "restaurant browser smoke captures screenshots" "frontend/scripts/smoke-restaurant-ordering.mjs" "OPENPR_RESTAURANT_SCREENSHOT_DIR"

printf '\nSmoke script coverage:\n'
for path in \
  scripts/smoke-universal-forms-api.sh \
  scripts/smoke-forms-mcp.sh \
  scripts/smoke-phase5-release-readiness.sh \
  scripts/smoke-plugins-mcp.sh \
  scripts/smoke-phase1-project-types-resources.sh \
  scripts/smoke-user-settings-api.sh \
  scripts/smoke-webhook-generic-consumer.sh \
  scripts/bootstrap-restaurant-demo.sh \
  scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh \
  scripts/smoke-wasm-plugin-runtime.sh \
  scripts/smoke-universal-forms-deployed-import.mjs \
  scripts/smoke-universal-forms-deployed-duplicate.mjs; do
  check_file "$path"
done
check_executable "scripts/smoke-universal-forms-deployed-import.mjs"
check_executable "scripts/smoke-universal-forms-deployed-permissions.mjs"
check_executable "scripts/smoke-universal-forms-deployed-duplicate.mjs"
contains "API smoke covers retry and idempotency" "scripts/smoke-universal-forms-api.sh" "retry and idempotency covered"
contains "deployed import smoke covers import preview endpoint" "scripts/smoke-universal-forms-deployed-import.mjs" "/records/import-preview"
contains "deployed import smoke covers import commit endpoint" "scripts/smoke-universal-forms-deployed-import.mjs" "/records/import"
contains "deployed import smoke covers browser import modal" "scripts/smoke-universal-forms-deployed-import.mjs" "导入记录"
contains "deployed duplicate smoke covers duplicate endpoint" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "/duplicate"
contains "deployed duplicate smoke covers MCP duplicate tool" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "forms.duplicate"
contains "deployed duplicate smoke covers browser duplicate action" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "复制表单"
contains "deployed duplicate smoke covers records not copied" "scripts/smoke-universal-forms-deployed-duplicate.mjs" "records should not be copied"
contains "deployed permissions smoke covers permissions endpoint" "scripts/smoke-universal-forms-deployed-permissions.mjs" "/permissions"
contains "deployed permissions smoke covers member denial" "scripts/smoke-universal-forms-deployed-permissions.mjs" "member create"
contains "deployed permissions smoke covers form list hiding" "scripts/smoke-universal-forms-deployed-permissions.mjs" "member list should hide form when form.view is false"
contains "deployed permissions smoke covers browser permissions panel" "scripts/smoke-universal-forms-deployed-permissions.mjs" "保存权限"
contains "API smoke covers form definition write events" "scripts/smoke-universal-forms-api.sh" "form definition/view write business events"
contains "API smoke covers the legacy webhook surface" "scripts/smoke-universal-forms-api.sh" "Legacy webhook receiver"
contains "user settings smoke verifies profile update" "scripts/smoke-user-settings-api.sh" "/api/v1/auth/me/profile"
contains "user settings smoke verifies preference persistence" "scripts/smoke-user-settings-api.sh" "/api/v1/auth/me/preferences"
contains "user settings smoke verifies password rotation" "scripts/smoke-user-settings-api.sh" "old password login should fail"
contains "AI task service emits business events" "apps/api/src/services/ai_task_service.rs" "ai_task.created"
contains "MCP smoke covers generic CLI" "scripts/smoke-forms-mcp.sh" "tools call"
contains "MCP smoke covers forms.duplicate" "scripts/smoke-forms-mcp.sh" "forms.duplicate"
contains "Phase 5 release readiness smoke covers MCP readiness" "scripts/smoke-phase5-release-readiness.sh" "release.readiness.get"
contains "project release readiness schema exists" "scripts/schemas/openpr-project-release-readiness.schema.json" "openpr.project.release_readiness.v1"
contains "project release readiness schema pins next action shape" "scripts/schemas/openpr-project-release-readiness.schema.json" '"next_actions"'
contains "project release readiness schema pins gate order" "scripts/schemas/openpr-project-release-readiness.schema.json" '"no_pending_ai_tasks"'
contains "project release readiness API emits schema version" "apps/api/src/routes/release_readiness.rs" "RELEASE_READINESS_SCHEMA_VERSION"
contains "project release readiness API emits schema path" "apps/api/src/routes/release_readiness.rs" "RELEASE_READINESS_SCHEMA_PATH"
contains "Phase 5 release readiness API emits next actions" "apps/api/src/routes/release_readiness.rs" "next_actions_for_gates"
contains "Phase 5 release readiness next actions expose order" "apps/api/src/routes/release_readiness.rs" "review_order"
contains "Phase 5 release readiness next actions expose blocking flag" "apps/api/src/routes/release_readiness.rs" "blocking"
contains "Phase 5 release readiness unit tests ready next action" "apps/api/src/routes/release_readiness.rs" "ready_release_readiness_returns_review_action"
contains "Phase 5 release readiness unit tests blocked next actions" "apps/api/src/routes/release_readiness.rs" "blocked_release_readiness_returns_gate_remediation_actions"
contains "MCP release readiness tool description documents next actions" "apps/mcp-server/src/tools/release.rs" "next actions"
contains "MCP release readiness tool unit tests next actions" "apps/mcp-server/src/tools/release.rs" "release_readiness_tool_documents_next_actions"
contains "Phase 5 release readiness smoke covers REST next actions" "scripts/smoke-phase5-release-readiness.sh" "ready release readiness should include reviewer next action"
contains "Phase 5 release readiness smoke covers schema version" "scripts/smoke-phase5-release-readiness.sh" "release readiness schema version should match schema"
contains "Phase 5 release readiness smoke covers schema path" "scripts/smoke-phase5-release-readiness.sh" "release readiness schema path should be stable"
contains "Phase 5 release readiness smoke covers MCP schema version" "scripts/smoke-phase5-release-readiness.sh" "MCP release.readiness.get should expose schema version"
contains "Phase 5 release readiness smoke covers MCP next actions" "scripts/smoke-phase5-release-readiness.sh" "MCP release.readiness.get should expose release next actions"
contains "Phase 5 release readiness smoke covers next action order" "scripts/smoke-phase5-release-readiness.sh" "action.review_order === 1"
contains "Phase 5 release readiness smoke covers next action blocking flag" "scripts/smoke-phase5-release-readiness.sh" "action.blocking === true"
contains "Phase 5 release readiness smoke covers blocked remediation action" "scripts/smoke-phase5-release-readiness.sh" "blocked release readiness should expose actionable pending AI task remediation"
contains "plugin MCP smoke covers formula patch" "scripts/smoke-plugins-mcp.sh" "formula"
contains "plugin MCP smoke covers plugin business events" "scripts/smoke-plugins-mcp.sh" "Plugin business events and outbox smoke passed"
contains "plugin MCP smoke covers automatic hook business events" "scripts/smoke-plugins-mcp.sh" "automatic hook plugin.invoked events"
contains "project type/resource smoke covers business events" "scripts/smoke-phase1-project-types-resources.sh" "Project type/resource business events and outbox smoke passed"
contains "project type/resource smoke covers project lifecycle events" "scripts/smoke-phase1-project-types-resources.sh" "Project lifecycle business events and outbox smoke passed"
contains "webhook smoke covers generic consumer" "scripts/smoke-webhook-generic-consumer.sh" "webhook generic consumer"
contains "restaurant demo bootstrap uses public API" "scripts/bootstrap-restaurant-demo.sh" "/api/v1/workspaces"
contains "restaurant demo bootstrap refuses remote API by default" "scripts/bootstrap-restaurant-demo.sh" "SYLVODE_DEMO_ALLOW_REMOTE"
contains "restaurant demo bootstrap verifies formula output" "scripts/bootstrap-restaurant-demo.sh" "order_line.line_total should be calculated from quantity and unit price"
contains "restaurant demo bootstrap creates MCP bot token" "scripts/bootstrap-restaurant-demo.sh" "/bots"
contains "restaurant demo bootstrap writes MCP config credentials" "scripts/bootstrap-restaurant-demo.sh" "mcp.workspace_id"
contains "restaurant demo bootstrap reloads the exact running MCP container" "scripts/bootstrap-restaurant-demo.sh" 'docker restart "${mcp_container_ids[0]}"'
contains "restaurant demo bootstrap verifies MCP HTTP projects.list" "scripts/bootstrap-restaurant-demo.sh" "projects.list"
contains "restaurant demo bootstrap checks demo project through MCP HTTP" "scripts/bootstrap-restaurant-demo.sh" "MCP HTTP verification passed"
contains "restaurant demo MCP HTTP smoke starts real MCP server" "scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "/target/debug/mcp-server\" --config"
contains "restaurant demo MCP HTTP smoke forces JSON-RPC verification" "scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "SYLVODE_DEMO_VERIFY_MCP_HTTP=1"
contains "restaurant demo MCP HTTP smoke proves RESTDEMO over MCP" "scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh" "projects.list includes RESTDEMO"
contains "security scope audit exists" "scripts/audit-universal-forms-security-scope.sh" "Universal forms security scope audit"
contains "security scope audit runs cargo audit JSON" "scripts/audit-universal-forms-security-scope.sh" "cargo audit --json"
contains "security scope audit proves PostgreSQL SQLx backend" "scripts/audit-universal-forms-security-scope.sh" "sqlx-postgres"
contains "security scope audit blocks the SeaORM MySQL backend" "scripts/audit-universal-forms-security-scope.sh" "workspace feature tree does not enable the SeaORM MySQL backend"
contains "security scope audit refuses a declared MySQL backend" "scripts/audit-universal-forms-security-scope.sh" "workspace does not declare the SeaORM MySQL backend"
contains "WASM runtime smoke runs plugin runtime tests" "scripts/smoke-wasm-plugin-runtime.sh" "plugins::runtime::tests::"
contains "WASM runtime tests cover fuel trap" "apps/api/src/plugins/runtime.rs" "fuel_limit_traps_runaway_plugins"

contains "MCP test script requires exact expected tool count" "scripts/test-mcp.sh" 'expected exactly $EXPECTED_TOOL_COUNT'
contains "MCP test script derives its strict default tool count" "scripts/test-mcp.sh" 'EXPECTED_TOOL_COUNT="${EXPECTED_TOOL_COUNT:-$(python3 "$PROJECT_ROOT/skills/sylvode-mcp/scripts/expected-tool-count.py")}"'
contains "MCP test script requires forms.duplicate" "scripts/test-mcp.sh" "forms.duplicate"
contains "MCP test script requires form metadata tools" "scripts/test-mcp.sh" "forms.schema_summary"
contains "MCP test script requires form permission tools" "scripts/test-mcp.sh" "form_permissions.get"
contains "MCP test script requires form attachment tools" "scripts/test-mcp.sh" "form_attachments.create"
contains "MCP test script requires relation tools" "scripts/test-mcp.sh" "form_records.relation_targets"
contains "MCP test script requires child lifecycle tools" "scripts/test-mcp.sh" "form_records.child_archive"
contains "MCP test script checks plugin invocations" "scripts/test-mcp.sh" "plugin_invocations.list"
contains "dev-up exposes PostgreSQL on localhost only" "scripts/dev-up.sh" "127.0.0.1:\${POSTGRES_PORT}:5432"
contains "dev-up prints host database url for the config file" "scripts/dev-up.sh" "url = \\\"postgres://openpr:\${POSTGRES_PASSWORD}@127.0.0.1:\${POSTGRES_PORT}/openpr\\\""
contains "init-db defaults to localhost development database" "scripts/init-db.sh" 'PGHOST="${PGHOST:-127.0.0.1}"'
contains "init-db defaults to development password" "scripts/init-db.sh" 'PGPASSWORD="${PGPASSWORD:-openpr_dev_password}"'

# Tool lists, counts and the skill text are generated from the tool catalogue; the generator is the
# check (it replaces the hand-written greps for the guide, the AGENTS and README tool lists).
if cargo run -q --locked -p mcp-server --bin sylvode-skillgen -- --check >/dev/null 2>&1; then
  printf 'PASS: skill and generated tool blocks match the tool catalogue (sylvode-skillgen --check)\n'
else
  printf 'FAIL: skill and generated tool blocks are out of date; run cargo run -p mcp-server --bin sylvode-skillgen -- --write\n' >&2
  failures=$((failures + 1))
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms source coverage audit failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms source coverage audit passed.\n'
