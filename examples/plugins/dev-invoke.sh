#!/usr/bin/env bash
# Runs a component through the temporary instance-admin route of the extension host.
#
#   SV_ADMIN_TOKEN=<instance admin access token> ./dev-invoke.sh <api-base-url> <component.wasm> <request.json>
#
# request.json holds the fields of `POST /api/v1/admin/extensions/dev-invoke` except `component_b64`:
# workspace_id, as_user_id (the user the plugin acts as), world (action | field-rule), entry (action
# name or rule stage), input (JSON text), grants [{capability, project?, form?}], requested?,
# project_id?, installation_id?. The answer is `{ok, output | error, record}`; the token is passed to curl
# through a header file descriptor, so it appears neither in the process list nor in the output.
set -euo pipefail

[ "$#" -eq 3 ] || { echo "usage: dev-invoke.sh <api-base-url> <component.wasm> <request.json>" >&2; exit 2; }
: "${SV_ADMIN_TOKEN:?set SV_ADMIN_TOKEN to an instance admin access token}"
command -v jq >/dev/null || { echo "jq is required" >&2; exit 2; }

base="${1%/}"
body="$(jq --arg component "$(base64 < "$2" | tr -d '\n')" '. + {component_b64: $component}' "$3")"
printf '%s' "$body" | curl --silent --show-error --fail-with-body \
  --header @<(printf 'Authorization: Bearer %s\n' "$SV_ADMIN_TOKEN") \
  --header "Content-Type: application/json" \
  --data-binary @- \
  "${base}/api/v1/admin/extensions/dev-invoke"
echo
