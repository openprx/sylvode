#!/bin/bash
set -euo pipefail

# Alert chain smoke test: registers an inbound source and rules through the management API, posts
# signed events with curl and openssl, and checks the work item, the task, the notifications, the
# idempotency and the safety limits through the API.
#
# Needs a running API with `[inbound] enabled = true` and a worker, an administrator of the
# workspace, a project, and a bot of that workspace. For the task to run to its end the bot needs an
# active webhook subscription and a running receiver (SV_EXPECT_COMPLETION=1 waits for it).
#
# Environment:
#   SV_API_URL              API base URL. Default: http://localhost:8081
#   SV_ADMIN_EMAIL          Sign-in of a workspace owner or admin. Required.
#   SV_ADMIN_PASSWORD       Its password. Required.
#   SV_PROJECT_ID           Project the alerts land in. Required.
#   SV_BOT_ID               User id of the bot the rule hands alerts to. Required.
#   SV_EXPECT_COMPLETION    1 to wait for the bot's task to complete and for the
#                           alert_task_completed notification. Default: 0
#   SV_COMPLETION_WAIT_SECS How long to wait for it. Default: 300
#
# Everything the script creates is removed again on exit. Secrets are never printed.
#
# Exit codes:
#   0  every check passed
#   1  a check failed
#   2  local usage or tool error

API="${SV_API_URL:-http://localhost:8081}"
EXPECT_COMPLETION="${SV_EXPECT_COMPLETION:-0}"
COMPLETION_WAIT="${SV_COMPLETION_WAIT_SECS:-300}"
RUN_ID="$(openssl rand -hex 3)"
FAILED=0
CHECKS=0

usage_error() {
  echo "smoke-alert-chain: $1" >&2
  exit 2
}

for required_tool in curl openssl python3; do
  command -v "$required_tool" >/dev/null 2>&1 || usage_error "$required_tool is required"
done
for required_var in SV_ADMIN_EMAIL SV_ADMIN_PASSWORD SV_PROJECT_ID SV_BOT_ID; do
  [[ -n "${!required_var:-}" ]] || usage_error "$required_var is required"
done
PROJECT="$SV_PROJECT_ID"

pass() {
  CHECKS=$((CHECKS + 1))
  echo "ok   $1"
}

fail() {
  CHECKS=$((CHECKS + 1))
  FAILED=$((FAILED + 1))
  echo "FAIL $1" >&2
}

expect_equal() {
  # expect_equal <description> <expected> <actual>
  if [[ "$2" == "$3" ]]; then
    pass "$1"
  else
    fail "$1 (expected '$2', got '$3')"
  fi
}

# jget <python expression over `d`, the parsed JSON on stdin>
jget() {
  python3 -c '
import json, sys
d = json.load(sys.stdin)
try:
    value = eval(sys.argv[1])
except Exception:
    value = ""
print("" if value is None else value)
' "$1"
}

# api <method> <path> [json body]: prints the response body, never the credentials.
api() {
  local method="$1" path="$2" body="${3:-}"
  if [[ -n "$body" ]]; then
    curl -sS -X "$method" "$API$path" -H "Authorization: Bearer $JWT" \
      -H 'Content-Type: application/json' --data-binary "$body"
  else
    curl -sS -X "$method" "$API$path" -H "Authorization: Bearer $JWT"
  fi
}

# sign <secret> <timestamp> <body>: lower case hex HMAC-SHA256 over "<timestamp>.<body>".
sign() {
  printf '%s.%s' "$2" "$3" | openssl dgst -sha256 -hmac "$1" -r | awk '{print $1}'
}

# post_event <token> <secret> <timestamp> <body>: prints "<status> <body>".
post_event() {
  local token="$1" secret="$2" ts="$3" body="$4" mac out
  mac="$(sign "$secret" "$ts" "$body")"
  out="$(curl -sS -o /dev/stdout -w ' %{http_code}' -X POST "$API/api/v1/inbound/events" \
    -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    -H "X-Sylvode-Timestamp: $ts" -H "X-Sylvode-Signature: v1=$mac" --data-binary "$body")"
  echo "${out##* } ${out% *}"
}

status_of() { echo "${1%% *}"; }
body_of() { echo "${1#* }"; }

event_json() {
  # event_json <event_id> <kind> <severity> <dedup_key> <host> [summary]
  python3 -c '
import json, sys, datetime
event_id, kind, severity, dedup, host = sys.argv[1:6]
summary = sys.argv[6] if len(sys.argv) > 6 else "Sent by the alert chain smoke test"
print(json.dumps({
    "spec": "sylvode.inbound_event.v1",
    "event_id": event_id,
    "occurred_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "kind": kind,
    "severity": severity,
    "dedup_key": dedup,
    "title": "Smoke test: SQL injection burst",
    "summary": summary,
    "subject": {"host": host},
    "payload": {"requests": 1240},
}, separators=(",", ":")))
' "$@"
}

# wait_until <seconds> <command...>: polls once a second.
wait_until() {
  local limit="$1" i
  shift
  for ((i = 0; i < limit; i++)); do
    if "$@"; then
      return 0
    fi
    sleep 1
  done
  "$@"
}

CLEANUP_PATHS=()
cleanup() {
  local path
  # Rules first, then the sources they are bound to; the sources take their events with them.
  for path in "${CLEANUP_PATHS[@]+"${CLEANUP_PATHS[@]}"}"; do
    api DELETE "$path" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

# ---- sign in ---------------------------------------------------------------------------------

login_body="$(python3 -c 'import json,os;print(json.dumps({"email":os.environ["SV_ADMIN_EMAIL"],"password":os.environ["SV_ADMIN_PASSWORD"]}))')"
login_answer="$(curl -sS -X POST "$API/api/v1/auth/login" -H 'Content-Type: application/json' --data-binary "$login_body")"
JWT="$(echo "$login_answer" | jget 'd["data"]["tokens"]["access_token"]')"
ADMIN_ID="$(echo "$login_answer" | jget 'd["data"]["user"]["id"]')"
[[ -n "$JWT" && -n "$ADMIN_ID" ]] || usage_error "sign-in failed"

# ---- register sources and rules --------------------------------------------------------------

register_source() {
  # register_source <name> <extra json fields>: sets SRC_ID, SRC_TOKEN, SRC_SECRET.
  local answer
  answer="$(api POST "/api/v1/projects/$PROJECT/inbound-sources" "{\"name\":\"$1\"$2}")"
  SRC_ID="$(echo "$answer" | jget 'd["data"]["source"]["id"]')"
  SRC_TOKEN="$(echo "$answer" | jget 'd["data"]["token"]')"
  SRC_SECRET="$(echo "$answer" | jget 'd["data"]["signing_secret"]')"
  [[ -n "$SRC_ID" && -n "$SRC_TOKEN" && -n "$SRC_SECRET" ]] || usage_error "creating source $1 failed (is [inbound] enabled and the account an owner or admin?)"
  CLEANUP_PATHS+=("/api/v1/inbound-sources/$SRC_ID")
}

register_rule() {
  # register_rule <name> <source id> <kind pattern> <extra json fields>: sets RULE_ID.
  local answer
  answer="$(api POST "/api/v1/projects/$PROJECT/alert-rules" \
    "{\"name\":\"$1\",\"source_id\":\"$2\",\"kind_patterns\":[\"$3\"],\"min_severity\":\"high\",\"action\":\"work_item_and_task\",\"bot_user_id\":\"$SV_BOT_ID\",\"instructions\":\"Smoke test: look at {alert_identifier} and comment briefly.\",\"notify_user_ids\":[\"$ADMIN_ID\"]$4}")"
  RULE_ID="$(echo "$answer" | jget 'd["data"]["rule"]["id"]')"
  [[ -n "$RULE_ID" ]] || usage_error "creating rule $1 failed: $(echo "$answer" | jget 'd.get("message")')"
  CLEANUP_PATHS=("/api/v1/alert-rules/$RULE_ID" "${CLEANUP_PATHS[@]+"${CLEANUP_PATHS[@]}"}")
}

NAME_MAIN="smoke-$RUN_ID-main"
NAME_CAP="smoke-$RUN_ID-cap"
NAME_RATE="smoke-$RUN_ID-rate"
register_source "$NAME_MAIN" ""
MAIN_ID="$SRC_ID" MAIN_TOKEN="$SRC_TOKEN" MAIN_SECRET="$SRC_SECRET"
register_rule "smoke main $RUN_ID" "$MAIN_ID" "waf.*" ',"rearm_after_secs":0'
register_source "$NAME_CAP" ""
CAP_ID="$SRC_ID" CAP_TOKEN="$SRC_TOKEN" CAP_SECRET="$SRC_SECRET"
register_rule "smoke cap $RUN_ID" "$CAP_ID" "cap.*" ',"max_tasks_per_hour":1'
register_source "$NAME_RATE" ""
RATE_TOKEN="$SRC_TOKEN" RATE_SECRET="$SRC_SECRET"
pass "registered three sources and two rules"

now() { date +%s; }

# ---- 1. a signed high event opens an alert, a work item and a task -----------------------------

HOST="smoke-$RUN_ID.example.com"
DEDUP="waf.attack_detected:$HOST"
EVENT1="$(event_json "smoke-$RUN_ID-1" waf.attack_detected high "$DEDUP" "$HOST")"
answer="$(post_event "$MAIN_TOKEN" "$MAIN_SECRET" "$(now)" "$EVENT1")"
expect_equal "a signed high event is accepted (202)" 202 "$(status_of "$answer")"
FIRST_EVENT_ID="$(body_of "$answer" | jget 'd["data"]["inbound_event_id"]')"

find_item() {
  api GET "/api/v1/projects/$PROJECT/issues?per_page=50&search=$1" | python3 -c '
import json, sys
d = json.load(sys.stdin)["data"]
items = d.get("items", d) if isinstance(d, dict) else d
mine = [i for i in items if sys.argv[1] in i.get("title", "")]
print(json.dumps(mine))
' "$1"
}

item_exists() { [[ "$(find_item "$NAME_MAIN" | jget 'len(d)')" -ge 1 ]]; }
wait_until 5 item_exists || true
items="$(find_item "$NAME_MAIN")"
expect_equal "one alert work item exists within 5 s" 1 "$(echo "$items" | jget 'len(d)')"
ITEM_ID="$(echo "$items" | jget 'd[0]["id"]')"
expect_equal "its title is built from Sylvode values" \
  "[HIGH] $NAME_MAIN · waf.attack_detected · $HOST" "$(echo "$items" | jget 'd[0]["title"]')"
expect_equal "its priority follows the severity" high "$(echo "$items" | jget 'd[0]["priority"]')"
expect_equal "it is assigned to the rule's bot" "$SV_BOT_ID" "$(echo "$items" | jget 'd[0]["assignee_id"]')"
expect_equal "its author is the administrator who registered the source" "$ADMIN_ID" "$(echo "$items" | jget 'd[0]["created_by"]')"

item_tasks() {
  api GET "/api/v1/projects/$PROJECT/ai/tasks?per_page=100" | python3 -c '
import json, sys
d = json.load(sys.stdin)["data"]
items = d.get("items", d) if isinstance(d, dict) else d
print(json.dumps([t for t in items if t.get("reference_id") == sys.argv[1]]))
' "$ITEM_ID"
}
tasks="$(item_tasks)"
expect_equal "one issue_assigned task exists for the work item" 1 "$(echo "$tasks" | jget 'len([t for t in d if t.get("task_type") == "issue_assigned"])')"
TASK_ID="$(echo "$tasks" | jget 'd[0]["id"]')"

count_notifications() {
  # count_notifications <type> <text the content must contain>
  api GET "/api/v1/notifications?limit=100" | python3 -c '
import json, sys
d = json.load(sys.stdin)["data"]
items = d.get("items", d.get("notifications", d)) if isinstance(d, dict) else d
print(len([n for n in items if n.get("type") == sys.argv[1] and sys.argv[2] in n.get("content", "")]))
' "$1" "$2"
}
expect_equal "one alert_opened notification for the rule's recipient" 1 "$(count_notifications alert_opened "$NAME_MAIN")"

# ---- 2. idempotency and deduplication ---------------------------------------------------------

answer="$(post_event "$MAIN_TOKEN" "$MAIN_SECRET" "$(now)" "$EVENT1")"
expect_equal "the same body again is a duplicate (200)" 200 "$(status_of "$answer")"
expect_equal "the duplicate names the stored event" "$FIRST_EVENT_ID" "$(body_of "$answer" | jget 'd["data"]["inbound_event_id"]')"
CHANGED="$(event_json "smoke-$RUN_ID-1" waf.attack_detected high "$DEDUP" "$HOST" "A different body with the same event id")"
answer="$(post_event "$MAIN_TOKEN" "$MAIN_SECRET" "$(now)" "$CHANGED")"
expect_equal "the same event id with another body is a conflict (409)" 409 "$(status_of "$answer")"
EVENT2="$(event_json "smoke-$RUN_ID-2" waf.attack_detected high "$DEDUP" "$HOST")"
answer="$(post_event "$MAIN_TOKEN" "$MAIN_SECRET" "$(now)" "$EVENT2")"
expect_equal "a new event id with the same dedup key is accepted (202)" 202 "$(status_of "$answer")"
expect_equal "it attaches to the open alert: still one work item" 1 "$(find_item "$NAME_MAIN" | jget 'len(d)')"
expect_equal "it starts no second task while the first is active" 1 "$(item_tasks | jget 'len(d)')"
occurrences="$(api GET "/api/v1/inbound-sources/$MAIN_ID/events?limit=10" | jget 'len([e for e in d["data"] if e["outcome"] == "alert_attached"])')"
expect_equal "the stored event is marked as attached" 1 "$occurrences"
expect_equal "an attach does not notify again" 1 "$(count_notifications alert_opened "$NAME_MAIN")"

# ---- 3. credentials and signatures ------------------------------------------------------------

EVENT3="$(event_json "smoke-$RUN_ID-3" waf.attack_detected high "waf.attack_detected:other-$RUN_ID" "$HOST")"
answer="$(post_event "$MAIN_TOKEN" "svs_$(openssl rand -hex 32)" "$(now)" "$EVENT3")"
expect_equal "a wrong signature is refused (401)" 401 "$(status_of "$answer")"
answer="$(post_event "$MAIN_TOKEN" "$MAIN_SECRET" "$(($(now) - 400))" "$EVENT3")"
expect_equal "an expired timestamp is refused (401)" 401 "$(status_of "$answer")"
answer="$(post_event "opr_$(openssl rand -hex 32)" "$MAIN_SECRET" "$(now)" "$EVENT3")"
expect_equal "a bot token shape is refused (401)" 401 "$(status_of "$answer")"
answer="$(post_event "svi_$(openssl rand -hex 32)" "$MAIN_SECRET" "$(now)" "$EVENT3")"
expect_equal "an unknown source token is refused (401)" 401 "$(status_of "$answer")"
expect_equal "refused requests store nothing" 2 \
  "$(api GET "/api/v1/inbound-sources/$MAIN_ID/events?limit=50" | jget 'len(d["data"])')"

# ---- 4. the per-rule task cap -----------------------------------------------------------------

CAP_HOST="cap-$RUN_ID.example.com"
for n in 1 2; do
  body="$(event_json "smoke-$RUN_ID-cap-$n" cap.attack_detected high "cap.attack_detected:$n:$CAP_HOST" "$CAP_HOST")"
  answer="$(post_event "$CAP_TOKEN" "$CAP_SECRET" "$(now)" "$body")"
  expect_equal "cap event $n is accepted (202)" 202 "$(status_of "$answer")"
done
cap_items() { find_item "$NAME_CAP"; }
expect_equal "both cap alerts opened a work item" 2 "$(cap_items | jget 'len(d)')"
outcomes="$(api GET "/api/v1/inbound-sources/$CAP_ID/events?limit=10" | jget 'sorted(e["outcome"] for e in d["data"])')"
expect_equal "the second alert's task was suppressed by the cap" "['alert_opened', 'suppressed_task_cap']" "$outcomes"
expect_equal "one alert_suppressed notification was written" 1 "$(count_notifications alert_suppressed "smoke cap $RUN_ID")"

# ---- 5. the per-source allowance --------------------------------------------------------------

# Events of a kind no rule matches are only stored, so 120 of them cost nothing but the requests.
last_status=""
for ((n = 1; n <= 121; n++)); do
  body="$(event_json "smoke-$RUN_ID-rate-$n" misc.note info "misc.note:$n" "rate-$RUN_ID.example.com")"
  answer="$(post_event "$RATE_TOKEN" "$RATE_SECRET" "$(now)" "$body")"
  last_status="$(status_of "$answer")"
  if ((n <= 120)) && [[ "$last_status" != "202" ]]; then
    fail "event $n of the allowance was refused ($last_status)"
    break
  fi
done
expect_equal "the 121st event within a minute is refused (429)" 429 "$last_status"

# ---- 6. the end of the task -------------------------------------------------------------------

if [[ "$EXPECT_COMPLETION" == "1" ]]; then
  task_done() {
    [[ "$(item_tasks | jget '[t["status"] for t in d if t["id"] == "'"$TASK_ID"'"][0]')" == "completed" ]]
  }
  if wait_until "$COMPLETION_WAIT" task_done; then
    pass "the bot's task completed"
    completion_notified() { [[ "$(count_notifications alert_task_completed "[HIGH] $NAME_MAIN")" -ge 1 ]]; }
    if wait_until 5 completion_notified; then
      pass "one alert_task_completed notification within 5 s of completion"
    else
      fail "no alert_task_completed notification within 5 s of completion"
    fi
    expect_equal "exactly one completion notification" 1 "$(count_notifications alert_task_completed "[HIGH] $NAME_MAIN")"
  else
    fail "the bot's task did not complete within ${COMPLETION_WAIT}s (is the receiver running?)"
  fi
fi

echo
echo "$((CHECKS - FAILED)) of $CHECKS checks passed"
if ((FAILED > 0)); then
  exit 1
fi
