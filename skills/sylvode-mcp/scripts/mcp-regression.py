#!/usr/bin/env python3
"""Sylvode MCP core regression - 3 transports with snapshot-derived registry checks."""
import json, subprocess, requests, time, threading, sys, queue, base64, os, atexit, shutil, tempfile

EXPECTED_TOOL_COUNT = int(subprocess.check_output(
    [sys.executable, os.path.join(os.path.dirname(__file__), "expected-tool-count.py")],
    text=True,
).strip())

def _required(name):
    """A required setting from the environment; the script holds no credentials or instance ids."""
    value = os.environ.get(name, "").strip()
    if not value:
        sys.exit(
            f"error: {name} is not set. Set SYLVODE_MCP_REGRESSION_TOKEN (a bot token bound to the "
            "transport under test), SYLVODE_MCP_REGRESSION_WORKSPACE_ID and "
            "SYLVODE_MCP_REGRESSION_PROJECT_ID; optionally SYLVODE_MCP_REGRESSION_MCP_URL "
            "(default http://localhost:8090) and SYLVODE_MCP_REGRESSION_API_URL "
            "(default http://localhost:8081)."
        )
    return value

MCP_HTTP = os.environ.get("SYLVODE_MCP_REGRESSION_MCP_URL", "http://localhost:8090").rstrip("/")
API_URL = os.environ.get("SYLVODE_MCP_REGRESSION_API_URL", "http://localhost:8081").rstrip("/")
TOKEN = _required("SYLVODE_MCP_REGRESSION_TOKEN")
WS = _required("SYLVODE_MCP_REGRESSION_WORKSPACE_ID")
PID = _required("SYLVODE_MCP_REGRESSION_PROJECT_ID")
MCP_BIN = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))),
    "target", "release", "mcp-server",
)

# The MCP server reads no environment variables and refuses to start without a configuration
# file, so the stdio transport gets one instead of an env dict. It is written 0600 into a
# temporary directory removed at exit, because it carries the bot token.
_CONFIG_DIR = tempfile.mkdtemp(prefix="sylvode-mcp-regression-")
atexit.register(shutil.rmtree, _CONFIG_DIR, True)
MCP_CONFIG = os.path.join(_CONFIG_DIR, "sylvode.toml")
with open(os.open(MCP_CONFIG, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w", encoding="utf-8") as _handle:
    _handle.write(
        '[logging]\nfilter = "error"\nformat = "text"\n\n'
        f'[mcp]\napi_url = "{API_URL}"\n'
        f'bot_token = "{TOKEN}"\nworkspace_id = "{WS}"\ntransport = "stdio"\n'
    )

# The registry checks below call registry tools by name, which is the opt-in "full" tool mode.
# The default mode ("search") is checked by search_mode_checks().
FULL_TOOLS_ARGS = ["--tools", "full"]

PASS = FAIL = SKIP = 0
ERRORS = []

def extract(resp):
    if isinstance(resp, dict):
        content = resp.get("result", {}).get("content", [])
        if content and "text" in content[0]:
            try: return json.loads(content[0]["text"])
            except: return content[0]["text"]
    return resp

def http_call(tool, args=None):
    try:
        r = requests.post(f"{MCP_HTTP}/mcp/rpc?tools=full", json={"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args or {}}}, timeout=15)
        return extract(r.json())
    except Exception as e: return {"error": str(e)}

def stdio_call(tool, args=None):
    payload = json.dumps({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args or {}}})
    try:
        env = {"PATH":"/usr/bin:/bin"}
        proc = subprocess.run([MCP_BIN,"serve","--transport","stdio","--config",MCP_CONFIG] + FULL_TOOLS_ARGS, input=payload, capture_output=True, text=True, timeout=15, env=env)
        for line in proc.stdout.strip().split("\n"):
            line = line.strip()
            if line.startswith("{"):
                return extract(json.loads(line))
        return {"error": f"no JSON output"}
    except Exception as e: return {"error": str(e)}

def sse_call(tool, args=None):
    """SSE: keep connection alive, POST, read response from stream"""
    try:
        q = queue.Queue()
        ready = threading.Event()
        stop = threading.Event()
        
        def listen():
            try:
                r = requests.get(f"{MCP_HTTP}/sse?tools=full", stream=True, headers={"Accept":"text/event-stream"}, timeout=30)
                event_type = None
                for line in r.iter_lines(decode_unicode=True):
                    if stop.is_set(): break
                    if line is None: continue
                    if line.startswith("event:"):
                        event_type = line[6:].strip()
                    elif line.startswith("data:"):
                        data = line[5:].strip()
                        q.put((event_type, data))
                        if event_type == "endpoint":
                            ready.set()
                        event_type = None
                    elif line == "":
                        event_type = None
            except: pass
        
        t = threading.Thread(target=listen, daemon=True)
        t.start()
        ready.wait(timeout=5)
        
        # Get endpoint
        endpoint = "/messages"
        try:
            evt_type, data = q.get(timeout=3)
            if evt_type == "endpoint":
                endpoint = data.strip()
        except: pass
        
        # POST
        url = f"{MCP_HTTP}{endpoint}" if endpoint.startswith("/") else endpoint
        resp = requests.post(url, json={"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args or {}}}, timeout=15)
        
        # Read message response from SSE stream
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                evt_type, data = q.get(timeout=2)
                if evt_type == "message":
                    stop.set()
                    return extract(json.loads(data))
            except: continue
        
        stop.set()
        return {"error": "no SSE response within 10s"}
    except Exception as e: return {"error": str(e)}

CALLERS = {"HTTP": http_call, "STDIO": stdio_call, "SSE": sse_call}

REGISTRY_NAMES = set(subprocess.check_output(
    [sys.executable, os.path.join(os.path.dirname(__file__), "expected-tool-count.py"), "--names"],
    text=True,
).split())

def http_list_tools():
    try:
        r = requests.post(f"{MCP_HTTP}/mcp/rpc?tools=full", json={"jsonrpc":"2.0","id":1,"method":"tools/list"}, timeout=15)
        return r.json()
    except Exception as e: return {"error": str(e)}

def stdio_list_tools():
    payload = json.dumps({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    try:
        env = {"PATH":"/usr/bin:/bin"}
        proc = subprocess.run([MCP_BIN,"serve","--transport","stdio","--config",MCP_CONFIG] + FULL_TOOLS_ARGS, input=payload, capture_output=True, text=True, timeout=15, env=env)
        for line in proc.stdout.strip().split("\n"):
            line = line.strip()
            if line.startswith("{"):
                return json.loads(line)
        return {"error": "no JSON output"}
    except Exception as e: return {"error": str(e)}

def sse_list_tools():
    try:
        q = queue.Queue()
        ready = threading.Event()
        stop = threading.Event()

        def listen():
            try:
                r = requests.get(f"{MCP_HTTP}/sse?tools=full", stream=True, headers={"Accept":"text/event-stream"}, timeout=30)
                event_type = None
                for line in r.iter_lines(decode_unicode=True):
                    if stop.is_set(): break
                    if line is None: continue
                    if line.startswith("event:"):
                        event_type = line[6:].strip()
                    elif line.startswith("data:"):
                        data = line[5:].strip()
                        q.put((event_type, data))
                        if event_type == "endpoint":
                            ready.set()
                        event_type = None
                    elif line == "":
                        event_type = None
            except: pass

        threading.Thread(target=listen, daemon=True).start()
        ready.wait(timeout=5)
        endpoint = "/messages"
        try:
            evt_type, data = q.get(timeout=3)
            if evt_type == "endpoint":
                endpoint = data.strip()
        except: pass
        url = f"{MCP_HTTP}{endpoint}" if endpoint.startswith("/") else endpoint
        requests.post(url, json={"jsonrpc":"2.0","id":1,"method":"tools/list"}, timeout=15)
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                evt_type, data = q.get(timeout=2)
                if evt_type == "message":
                    stop.set()
                    return json.loads(data)
            except: continue
        stop.set()
        return {"error": "no SSE response within 10s"}
    except Exception as e: return {"error": str(e)}

LISTERS = {"HTTP": http_list_tools, "STDIO": stdio_list_tools, "SSE": sse_list_tools}

def registry_matches_expected_tools_with_forms_and_plugins(resp):
    tools = resp.get("result", {}).get("tools", []) if isinstance(resp, dict) else []
    names = {tool.get("name") for tool in tools if isinstance(tool, dict)}
    return len(tools) == EXPECTED_TOOL_COUNT and names == REGISTRY_NAMES

def check(proto, tool, result, expect_fn):
    global PASS, FAIL
    try: ok = expect_fn(result)
    except: ok = False
    if ok:
        PASS += 1; print(f"  ✅ [{proto}] {tool}")
    else:
        FAIL += 1; s = str(result)[:140]; ERRORS.append(f"  ❌ [{proto}] {tool} → {s}"); print(f"  ❌ [{proto}] {tool} → {s}")
    return ok

def is_ok(r): return isinstance(r, dict) and r.get("code") == 0
def has_id(r): return is_ok(r) and r.get("data", {}).get("id")
def get_id(r):
    try: return r["data"]["id"]
    except: return None
def ok_or_str(r): return is_ok(r) or (isinstance(r, str) and any(w in r.lower() for w in ["added","removed","deleted","success"]))


SEARCH_MODE_TOOLS = {
    "sylvode_overview", "sylvode_search", "sylvode_read", "sylvode_write",
    "sylvode_task_get", "sylvode_task_ack", "sylvode_task_progress", "sylvode_task_complete", "sylvode_task_fail",
}

def _search_http(method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        body["params"] = params
    return requests.post(f"{MCP_HTTP}/mcp/rpc", json=body, timeout=15).json()

def _search_stdio(method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        body["params"] = params
    proc = subprocess.run([MCP_BIN, "serve", "--transport", "stdio", "--config", MCP_CONFIG],
                          input=json.dumps(body), capture_output=True, text=True, timeout=15, env={"PATH": "/usr/bin:/bin"})
    for line in proc.stdout.strip().split("\n"):
        if line.strip().startswith("{"):
            return json.loads(line)
    return {"error": "no JSON output"}

def _tool_text(resp):
    content = resp.get("result", {}).get("content", []) if isinstance(resp, dict) else []
    return content[0].get("text", "") if content else ""

def _is_tool_error(resp):
    return isinstance(resp, dict) and resp.get("result", {}).get("isError") is True

def search_mode_checks(proto, rpc):
    """The default tool mode: nine tools, search finds a tool with its schema, the wrappers route by access."""
    print(f"\n🔎 Search mode ({proto}, default)")
    def tools_call(name, arguments):
        return rpc("tools/call", {"name": name, "arguments": arguments})
    listed = {t.get("name") for t in rpc("tools/list").get("result", {}).get("tools", [])}
    check(proto, "search mode lists exactly the nine tools", listed, lambda names: names == SEARCH_MODE_TOOLS)
    found = json.loads(_tool_text(tools_call("sylvode_search", {"query": "work_items.get"})) or "{}")
    check(proto, "sylvode_search returns work_items.get first with its schema", found,
          lambda r: r.get("results", [{}])[0].get("name") == "work_items.get" and "inputSchema" in r["results"][0])
    check(proto, "sylvode_read projects.list succeeds",
          tools_call("sylvode_read", {"tool": "projects.list", "arguments": {}}), lambda r: not _is_tool_error(r))
    check(proto, "sylvode_read refuses work_items.create",
          tools_call("sylvode_read", {"tool": "work_items.create", "arguments": {}}), _is_tool_error)
    check(proto, "sylvode_write refuses projects.list",
          tools_call("sylvode_write", {"tool": "projects.list", "arguments": {}}), _is_tool_error)
    check(proto, "a direct registry call is refused in search mode",
          tools_call("projects.list", {}), _is_tool_error)

print("=" * 60)
# The banner renders the derived count, never a literal number.
print(f"  Sylvode MCP core regression ({EXPECTED_TOOL_COUNT} registered tools x 3 transports)")
print(f"  {time.strftime('%Y-%m-%d %H:%M:%S')}")
print("=" * 60)

search_mode_checks("HTTP", _search_http)
search_mode_checks("STDIO", _search_stdio)

for proto, call in CALLERS.items():
    print(f"\n{'━'*50}\n  Transport: {proto}\n{'━'*50}")

    check(
        proto,
        f"tools/list.registry_{EXPECTED_TOOL_COUNT}",
        LISTERS[proto](),
        registry_matches_expected_tools_with_forms_and_plugins,
    )
    
    # === Projects ===
    print("\n📁 Projects (list/get)")
    check(proto, "projects.list", call("projects.list"), is_ok)
    check(proto, "projects.get", call("projects.get", {"project_id": PID}), is_ok)

    # === Project Types / Resources ===
    print("\n🧭 Project Types & Resources")
    check(proto, "project_types.list", call("project_types.list"), is_ok)
    check(proto, "project_types.get", call("project_types.get", {"key": "code_project"}), is_ok)
    check(proto, "scenario_templates.list", call("scenario_templates.list"), is_ok)
    check(
        proto,
        "scenario_templates.get",
        call("scenario_templates.get", {"key": "contract_review_default"}),
        is_ok,
    )

    # === Context ===
    print("\n🧠 Context")
    check(proto, "context.get_project", call("context.get_project", {"project_id": PID}), is_ok)
    check(proto, "context.get_governance", call("context.get_governance", {"project_id": PID}), is_ok)
    check(proto, "context.get_agent_policy", call("context.get_agent_policy", {"project_id": PID}), is_ok)

    # === Work Items: read ===
    print("\nWork Items (read: list/get/search/get_by_identifier)")
    check(proto, "work_items.list", call("work_items.list", {"project_id": PID}), is_ok)
    check(proto, "work_items.get", call("work_items.get", {"work_item_id": "40b28aac-97d4-4bb7-adbb-f1db6bb763e8"}), is_ok)
    check(proto, "work_items.search", call("work_items.search", {"query": "test"}), is_ok)
    check(proto, "work_items.get_by_identifier", call("work_items.get_by_identifier", {"identifier": "ADMIN123-1"}), lambda r: isinstance(r, dict))

    # === Work Items: write ===
    print("\nWork Items (write: create/update/labels/delete)")
    wi = call("work_items.create", {"project_id": PID, "title": f"{proto}-final-regtest", "priority": "low", "state": "backlog"})
    wi_ok = check(proto, "work_items.create", wi, has_id)
    wi_id = get_id(wi)
    if wi_id:
        check(proto, "work_items.update", call("work_items.update", {"work_item_id": wi_id, "state": "in_progress", "priority": "high"}), is_ok)
        check(proto, "work_items.add_label", call("work_items.add_label", {"work_item_id": wi_id, "label_id": "7c466d81-09ea-41d7-8513-b39c710c8330"}), ok_or_str)
        check(proto, "work_items.add_labels", call("work_items.add_labels", {"work_item_id": wi_id, "label_ids": ["94b44a22-01c9-4cbc-8208-b01fdd1c68a8"]}), ok_or_str)
        check(proto, "work_items.list_labels", call("work_items.list_labels", {"work_item_id": wi_id}), is_ok)
        check(proto, "work_items.remove_label", call("work_items.remove_label", {"work_item_id": wi_id, "label_id": "94b44a22-01c9-4cbc-8208-b01fdd1c68a8"}), ok_or_str)
        
        # === Comments ===
        print(f"\n💬 Comments (create/list/delete)")
        cmt = call("comments.create", {"work_item_id": wi_id, "content": f"{proto} final regression comment"})
        check(proto, "comments.create", cmt, has_id)
        cmt_id = get_id(cmt)
        check(proto, "comments.list", call("comments.list", {"work_item_id": wi_id}), is_ok)
        if cmt_id: check(proto, "comments.delete", call("comments.delete", {"comment_id": cmt_id}), ok_or_str)
        else: SKIP += 1
        
        check(proto, "work_items.delete", call("work_items.delete", {"work_item_id": wi_id}), ok_or_str)
    else:
        SKIP += 10; print(f"  ⏭️  Skipping write tests (create failed)")

    # === Files Upload ===
    print(f"\n📎 Files (upload)")
    b64 = base64.b64encode(b"regression test log content").decode()
    fup = call("files.upload", {"filename": f"{proto}-test.log", "content_base64": b64})
    file_ok = check(proto, "files.upload", fup, lambda r: isinstance(r, dict) and "url" in r)

    # === Labels ===
    print(f"\n🏷️ Labels (list/list_by_project/create/update/delete)")
    check(proto, "labels.list", call("labels.list"), is_ok)
    check(proto, "labels.list_by_project", call("labels.list_by_project", {"project_id": PID}), is_ok)
    ts = int(time.time()) % 100000
    lbl = call("labels.create", {"name": f"{proto}-final-{ts}", "color": "#cc5500"})
    check(proto, "labels.create", lbl, has_id)
    lbl_id = get_id(lbl)
    if lbl_id:
        check(proto, "labels.update", call("labels.update", {"label_id": lbl_id, "name": f"{proto}-upd-{ts}"}), is_ok)
        check(proto, "labels.delete", call("labels.delete", {"label_id": lbl_id}), ok_or_str)
    else: SKIP += 2

    # === Members ===
    print(f"\n👥 Members (list)")
    check(proto, "members.list", call("members.list"), is_ok)

    # === Sprints ===
    print(f"\n🏃 Sprints (list/create/update/delete)")
    check(proto, "sprints.list", call("sprints.list", {"project_id": PID}), is_ok)
    spr = call("sprints.create", {"project_id": PID, "name": f"{proto}-final-spr", "start_date": "2026-04-01", "end_date": "2026-04-14"})
    check(proto, "sprints.create", spr, has_id)
    spr_id = get_id(spr)
    if spr_id:
        check(proto, "sprints.update", call("sprints.update", {"sprint_id": spr_id, "name": f"{proto}-spr-final-upd"}), is_ok)
        check(proto, "sprints.delete", call("sprints.delete", {"sprint_id": spr_id}), ok_or_str)
    else: SKIP += 2

    # === Proposals / Check Results ===
    print(f"\n📝 Proposals & Check Results (list/get/check result/proposal)")
    check(proto, "proposals.list", call("proposals.list", {"project_id": PID}), is_ok)
    check(proto, "proposals.get", call("proposals.get", {"proposal_id": "PROP-053d48a2-1b85-409a-9b65-2a99281cbcef"}), lambda r: isinstance(r, dict) and "code" in r)
    chk = call("check_results.create", {
        "project_id": PID,
        "action_class": "high_risk_mutation",
        "risk_level": "high",
        "title": f"{proto}-regression governed check result",
        "summary": f"{proto} regression created a high-risk check result for proposal conversion validation.",
        "result": {"source": "mcp-regression", "protocol": proto}
    })
    check(proto, "check_results.create", chk, has_id)
    chk_id = get_id(chk)
    if chk_id:
        proposal = call("proposals.create_from_result", {
            "check_result_id": chk_id,
            "title": f"{proto}-regression proposal from check result",
            "domains": ["governance", "regression"],
            "submit": False
        })
        check(proto, "proposals.create_from_result", proposal, has_id)
    else:
        SKIP += 1; print(f"  ⏭️  [{proto}] proposals.create_from_result (check result create failed)")
    check(proto, "release.readiness.get", call("release.readiness.get", {"project_id": PID}), is_ok)

    # === Scenario Tools ===
    print(f"\n🏭 Scenario tools (code/traditional governed entries)")
    check(proto, "code.task_context.get", call("code.task_context.get", {"project_id": PID}), lambda r: isinstance(r, dict) or isinstance(r, str))
    for tool, title, summary in [
        ("code.change_proposal.create", "Code change proposal", "Governed code change proposal from regression."),
        ("documents.extract_summary", "Document summary", "Governed document summary from regression."),
        ("documents.review_risk", "Document risk review", "Governed document risk review from regression."),
        ("approval.request", "Approval request", "Governed approval request from regression."),
        ("inspection.report", "Inspection report", "Governed inspection report from regression."),
        ("corrective_action.propose", "Corrective action", "Governed corrective action proposal from regression."),
    ]:
        result = call(tool, {
            "project_id": PID,
            "title": f"{proto}-{title}",
            "summary": f"{proto} {summary}",
            "result": {"source": "mcp-regression", "protocol": proto, "tool": tool}
        })
        check(proto, tool, result, has_id)

    # === Search ===
    print(f"\n🔍 Search (all)")
    check(proto, "search.all", call("search.all", {"query": "test"}), is_ok)

# Summary
total_per_proto = PASS + FAIL  # approximate
print(f"\n{'='*60}")
print("  Final result")
print(f"{'='*60}")
print(f"  Passed:  {PASS}")
print(f"  Failed:  {FAIL}")
print(f"  Skipped: {SKIP}")
print(f"  Pass rate: {PASS*100//(PASS+FAIL) if (PASS+FAIL) else 0}%")
if ERRORS:
    print("\n  Failures:")
    for e in ERRORS: print(e)
print(f"{'='*60}")
sys.exit(1 if FAIL > 0 else 0)
