#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_PORT="${OPENPR_SMOKE_PG_PORT:-5366}"
PG_SUPERUSER="${OPENPR_SMOKE_PG_SUPERUSER:-postgres}"
DB_NAME="sylvode_scenario_forms_smoke_$$_$(date +%s)"
DB_USER="sylvode_scenario_forms_smoke_$$_$(date +%s)"
DB_PASSWORD="$(openssl rand -hex 12)"
API_PORT="${OPENPR_SMOKE_API_PORT:-$((22180 + ($$ % 1000)))}"
TMP_DIR="$(mktemp -d /tmp/sylvode-scenario-forms-smoke.XXXXXX)"
API_LOG="$TMP_DIR/api.log"
SMOKE_JWT_SECRET="sylvode-scenario-forms-smoke-secret"

OWNER_ID="11111111-1111-4111-8111-111111111111"
WORKSPACE_ID="33333333-3333-4333-8333-333333333333"

api_pid=""

cleanup() {
  local exit_code=$?
  if [[ -n "$api_pid" ]] && kill -0 "$api_pid" 2>/dev/null; then
    kill "$api_pid" 2>/dev/null || true
    wait "$api_pid" 2>/dev/null || true
  fi
  sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL >/dev/null 2>&1 || true
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$DB_NAME';
DROP DATABASE IF EXISTS "$DB_NAME";
DROP ROLE IF EXISTS "$DB_USER";
SQL
  if [[ $exit_code -ne 0 ]]; then
    echo "Smoke failed. API log: $API_LOG" >&2
  else
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 2
  }
}

wait_http() {
  local url="$1"
  local name="$2"
  for _ in $(seq 1 120); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for $name at $url" >&2
  return 1
}

psql_smoke() {
  PGPASSWORD="$DB_PASSWORD" psql -h 127.0.0.1 -p "$POSTGRES_PORT" -U "$DB_USER" -d "$DB_NAME" "$@"
}

require_cmd curl
require_cmd node
require_cmd openssl
require_cmd psql
require_cmd sudo

cargo build -q -p api --bin api

sudo -n -u "$PG_SUPERUSER" psql -p "$POSTGRES_PORT" -d postgres -v ON_ERROR_STOP=1 -q <<SQL
CREATE ROLE "$DB_USER" LOGIN PASSWORD '$DB_PASSWORD';
CREATE DATABASE "$DB_NAME" OWNER "$DB_USER";
SQL

SMOKE_DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$POSTGRES_PORT/$DB_NAME"

# The binaries read no environment variables; this file is their only configuration, and they
# refuse to start without one. It is written inside the 0700 directory mktemp made for this run
# and is removed with it, so the generated database password never lands in the repository.
# text logging rather than json: the only reader of the log file is a human debugging a failure.
APP_CONFIG="$TMP_DIR/sylvode.toml"
cat >"$APP_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$SMOKE_DATABASE_URL"

[auth]
jwt_secret = "$SMOKE_JWT_SECRET"

[logging]
filter = "${OPENPR_SMOKE_LOG_FILTER:-api=info}"
format = "text"
EOF

"$ROOT_DIR/target/debug/api" --config "$APP_CONFIG" >"$API_LOG" 2>&1 &
api_pid=$!
wait_http "http://127.0.0.1:$API_PORT/health" "Sylvode API"

psql_smoke -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$OWNER_ID', 'scenario-forms-owner@example.local', '', 'Scenario Forms Owner', 'admin', true, 'human', NULL, now(), now());

INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'scenario-forms-smoke', 'Scenario Forms Smoke', '$OWNER_ID', now(), now());

INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE_ID', '$OWNER_ID', 'owner', now());
SQL

API_URL="http://127.0.0.1:$API_PORT" \
SMOKE_JWT_SECRET="$SMOKE_JWT_SECRET" \
OWNER_ID="$OWNER_ID" \
WORKSPACE_ID="$WORKSPACE_ID" \
node --input-type=commonjs <<'NODE'
const crypto = require('crypto');

const apiUrl = process.env.API_URL;
const workspaceId = process.env.WORKSPACE_ID;
const jwtSecret = process.env.SMOKE_JWT_SECRET;

function b64url(value) {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}

function jwt(sub, email) {
  const now = Math.floor(Date.now() / 1000);
  const header = b64url({ alg: 'HS256', typ: 'JWT' });
  const body = b64url({ sub, email, token_type: 'access', iat: now, exp: now + 3600 });
  const sig = crypto.createHmac('sha256', jwtSecret).update(`${header}.${body}`).digest('base64url');
  return `${header}.${body}.${sig}`;
}

const ownerToken = jwt(process.env.OWNER_ID, 'scenario-forms-owner@example.local');

async function request(method, path, body, expectedStatus = 200) {
  const response = await fetch(`${apiUrl}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${ownerToken}`,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (response.status !== expectedStatus) {
    throw new Error(`${method} ${path} expected HTTP ${expectedStatus}, got ${response.status}: ${text}`);
  }
  return payload;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function keys(items) {
  return items.map((item) => item.key).sort();
}

async function assertTemplateCreatesForms(templateKey, projectKey, expectedKeys) {
  const project = await request('POST', `/api/v1/workspaces/${workspaceId}/projects`, {
    name: `${templateKey} smoke`,
    key: projectKey,
    scenario_template_key: templateKey,
  });
  const projectId = project.data.id;
  assert(project.data.type_settings?.scenario_template_key === templateKey, `${templateKey} should be embedded in type_settings`);

  const forms = await request('GET', `/api/v1/projects/${projectId}/forms`);
  assert(JSON.stringify(keys(forms.data.items)) === JSON.stringify([...expectedKeys].sort()), `${templateKey} form keys mismatch: ${JSON.stringify(keys(forms.data.items))}`);

  for (const form of forms.data.items) {
    const views = await request('GET', `/api/v1/forms/${form.id}/views`);
    const viewKeys = keys(views.data);
    assert(viewKeys.includes('grid'), `${form.key} missing grid view`);
    assert(viewKeys.includes('detail'), `${form.key} missing detail view`);
  }

  return { projectId, forms: forms.data.items };
}

async function main() {
  const templates = await request('GET', '/api/v1/scenario-templates');
  const templateKeys = templates.data.items.map((item) => item.key);
  assert(templateKeys.includes('restaurant_ordering_default'), 'restaurant template should be exposed');
  const restaurantTemplateListItem = templates.data.items.find((item) => item.key === 'restaurant_ordering_default');
  assert(
    restaurantTemplateListItem.usage_guide?.schema_version === 'openpr.scenario_template.usage_guide.v1',
    'restaurant template list item should expose usage guide schema version',
  );
  assert(
    restaurantTemplateListItem.usage_guide.primary_mcp_tools.includes('projects.create'),
    'restaurant template list item should expose project creation MCP tool',
  );
  assert(
    restaurantTemplateListItem.usage_guide.primary_mcp_tools.includes('form_records.aggregate'),
    'restaurant template list item should expose aggregate MCP tool',
  );
  assert(
    restaurantTemplateListItem.usage_guide.plugin_keys.length === 0,
    'restaurant template list item should list no plugin key',
  );

  const restaurantTemplate = await request('GET', '/api/v1/scenario-templates/restaurant_ordering_default');
  assert(
    restaurantTemplate.data.usage_guide.operator_entrypoints.some((entrypoint) => entrypoint.key === 'mcp_projects_create'),
    'restaurant template detail should expose MCP project creation entrypoint',
  );
  assert(
    restaurantTemplate.data.usage_guide.acceptance_focus.some((focus) => focus.includes('Revenue aggregate')),
    'restaurant template detail should expose acceptance focus',
  );

  await assertTemplateCreatesForms('code_delivery_default', 'CODEFORM', [
    'code_task',
    'change_record',
    'release_check',
  ]);
  await assertTemplateCreatesForms('contract_review_default', 'CONTFORM', [
    'contract',
    'risk_clause',
    'approval_record',
  ]);
  await assertTemplateCreatesForms('equipment_maintenance_default', 'EQPFORM', [
    'equipment',
    'repair_order',
    'inspection_record',
  ]);
  await assertTemplateCreatesForms('quality_corrective_action_default', 'QCAFORM', [
    'defect_record',
    'root_cause_analysis',
    'corrective_action',
  ]);
  await assertTemplateCreatesForms('customer_delivery_default', 'DELIVFORM', [
    'customer',
    'delivery_milestone',
    'change_request',
  ]);

  const restaurant = await assertTemplateCreatesForms('restaurant_ordering_default', 'RESTFORM', [
    'menu_category',
    'sku',
    'table',
    'order',
    'order_line',
    'print_job',
    'business_report',
  ]);
  const plugins = await request('GET', `/api/v1/projects/${restaurant.projectId}/plugins`);
  assert(
    !plugins.data.items.some((plugin) => plugin.key === 'restaurant_calc'),
    'restaurant template should not install a restaurant_calc plugin',
  );

  const orderForm = restaurant.forms.find((form) => form.key === 'order');
  assert(orderForm.schema.fields.some((field) => field.key === 'total_amount' && field.type === 'amount'), 'order form should contain amount field');
  const orderLineForm = restaurant.forms.find((form) => form.key === 'order_line');
  assert(orderLineForm.schema.fields.some((field) => field.key === 'order_id' && field.type === 'relation'), 'order_line should relate to order');

  // The order line calculates its own total: quantity x unit price.
  const lineSchemaTotal = orderLineForm.schema.fields.find((field) => field.key === 'line_total');
  assert(
    lineSchemaTotal && lineSchemaTotal.formula && lineSchemaTotal.formula.op === 'multiply',
    'order_line line_total should be a formula field',
  );
  const table = await request('POST', `/api/v1/forms/${restaurant.forms.find((form) => form.key === 'table').id}/records`, {
    values: { table_no: 'A03' },
  });
  const sku = await request('POST', `/api/v1/forms/${restaurant.forms.find((form) => form.key === 'sku').id}/records`, {
    values: { sku: 'NOO-1', name: 'Noodles', price: '9.99' },
  });
  const order = await request('POST', `/api/v1/forms/${orderForm.id}/records`, {
    values: { order_no: 'SMOKE-1', table_id: table.data.id, status: 'draft' },
  });
  const line = await request('POST', `/api/v1/forms/${orderLineForm.id}/records`, {
    values: { order_id: order.data.id, sku_id: sku.data.id, sku_name: 'Noodles', quantity: 3, unit_price: '12.34' },
  });
  assert(line.data.values.line_total.decimal === '37.02', 'an order line should get line_total 3 x 12.34 = 37.02');

  console.log('scenario template forms smoke passed');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE

SCENARIO_PROJECT_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'project.created' AND metadata->>'scenario_template_initialized' = 'true'")"
if [[ "$SCENARIO_PROJECT_EVENT_COUNT" -lt 6 ]]; then
  echo "Expected at least 6 scenario project.created business events, got $SCENARIO_PROJECT_EVENT_COUNT" >&2
  exit 1
fi

SCENARIO_FORM_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'form.created' AND metadata->>'scenario_template_initialized' = 'true'")"
if [[ "$SCENARIO_FORM_EVENT_COUNT" -lt 22 ]]; then
  echo "Expected at least 22 scenario form.created business events, got $SCENARIO_FORM_EVENT_COUNT" >&2
  exit 1
fi

SCENARIO_VIEW_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'form.view.created' AND metadata->>'scenario_template_initialized' = 'true'")"
if [[ "$SCENARIO_VIEW_EVENT_COUNT" -lt 44 ]]; then
  echo "Expected at least 44 scenario form.view.created business events, got $SCENARIO_VIEW_EVENT_COUNT" >&2
  exit 1
fi

SCENARIO_PLUGIN_EVENT_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM business_events WHERE event_type = 'plugin.installed' AND metadata->>'scenario_template_initialized' = 'true' AND metadata->>'plugin_key' = 'restaurant_calc'")"
if [[ "$SCENARIO_PLUGIN_EVENT_COUNT" -ne 0 ]]; then
  echo "Expected no restaurant_calc scenario plugin.installed business event, got $SCENARIO_PLUGIN_EVENT_COUNT" >&2
  exit 1
fi

SCENARIO_OUTBOX_COUNT="$(psql_smoke -Atqc "SELECT COUNT(*) FROM event_outbox eo JOIN business_events be ON be.id = eo.business_event_id WHERE be.event_type IN ('project.created', 'form.created', 'form.view.created', 'plugin.installed') AND be.metadata->>'scenario_template_initialized' = 'true'")"
if [[ "$SCENARIO_OUTBOX_COUNT" -lt 72 ]]; then
  echo "Expected at least 72 scenario initialization outbox rows, got $SCENARIO_OUTBOX_COUNT" >&2
  exit 1
fi

echo "scenario template initialization business events and outbox smoke passed"
