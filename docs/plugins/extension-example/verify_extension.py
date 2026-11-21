#!/usr/bin/env python3
"""Verify the protocol 3 extension backend against an isolated API and a fresh PostgreSQL.

Run against a throwaway API started with SV_FORM_PLUGIN_PACKAGES_ENABLED=1,
SV_FORM_PLUGIN_EXTENSIONS_ENABLED=1:

    verify_extension.py --base http://127.0.0.1:18995 --context /private/path/context.json

The default phase covers the backend parts of acceptance cases X1-X18. It writes a private context
file that the follow-up phases read after the API is restarted:

    --phase gate-off   API restarted with SV_FORM_PLUGIN_EXTENSIONS_ENABLED=0 (X21)
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from secrets import token_urlsafe
import sys
import threading
import time
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen
from uuid import uuid4

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build as samples  # noqa: E402

BASE = "http://127.0.0.1:18995"
VERIFIED = []


def check(name):
    VERIFIED.append(name)


def call(method, path, payload=None, token=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = json.dumps(payload).encode() if payload is not None else None
    started = time.monotonic()
    try:
        with urlopen(Request(BASE + path, data=data, headers=headers, method=method), timeout=30) as response:
            body = json.load(response)
    except HTTPError as failure:
        body = json.load(failure)
    return body, started, time.monotonic()


def request(method, path, payload=None, token=None, expect=0, reason=None, error_code=None):
    body, _, _ = call(method, path, payload, token)
    assert body["code"] == expect, (method, path, expect, body)
    if reason is not None:
        assert (body.get("details") or {}).get("reason") == reason, (method, path, reason, body)
    if error_code is not None:
        assert body.get("error_code") == error_code, (method, path, error_code, body)
    return body.get("data")


def create_account(name, token=None):
    account = request("POST", "/api/v1/auth/register", {
        "email": f"{name}-{uuid4().hex[:8]}@plugin.test", "password": token_urlsafe(24), "name": name,
    }, token)
    return account["user"]["id"], account["tokens"]["access_token"]


def create_form(project, token):
    return request("POST", f"/api/v1/projects/{project}/forms", {
        "key": "order_line", "name": "Order line", "schema": {"fields": [
            {"key": "quantity", "label": "Quantity", "type": "number"},
            {"key": "unit_price", "label": "Unit price", "type": "number"},
            {"key": "note", "label": "Note", "type": "text"},
            {"key": "amount", "label": "Amount", "type": "number"},
        ]},
    }, token)["id"]


def create_record(form, token, quantity="2", price="12.50"):
    return request("POST", f"/api/v1/forms/{form}/records", {
        "title": "Order", "values": {"quantity": quantity, "unit_price": price, "amount": "0", "note": "keep"},
    }, token)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encode(data):
    return base64.b64encode(data).decode("ascii")


def entry_html():
    return (samples.EXAMPLE / "panel-view.html").read_bytes()


def ui_package(version="1.0.0", key="order_board"):
    html = entry_html()
    manifest = samples.base_manifest(key, "Order board", html, "order_line")
    manifest["version"] = version
    if version == "1.1.0":
        manifest["contributions"] = [
            item for item in manifest["contributions"] if item["slot"] != "form.list.toolbar"
        ] + [samples.contribution("record_action", "action", "form.record.actions", "Order action",
                                  "action", None, "order_line")]
    return {"manifest": manifest, "web_base64": encode(html)}


def wasm_package(wasm):
    html = entry_html()
    manifest = samples.base_manifest("order_amount_v3", "Order amount", html, "order_line")
    manifest["profile"] = "wasm"
    manifest["wasm"] = {"abi": "openpr.plugin.v1", "path": "plugin.wasm", "size": len(wasm),
                        "sha256": digest(wasm), "form_key": "order_line"}
    manifest["capabilities"] = [samples.capability("amount.preview", "record.read"),
                                samples.capability("amount.apply", "record.update")]
    manifest["contributions"] = [
        samples.contribution("detail", "panel", "form.record.detail", "Amount", "summary", None, "order_line"),
        samples.contribution("actions", "action", "form.record.actions", "Calculate", "totals", None, "order_line"),
        samples.contribution("view", "view", "project.view", "Amounts", "board", "amounts", None),
    ]
    return {"manifest": manifest, "web_base64": encode(html), "wasm_base64": encode(wasm)}


class Project:
    def __init__(self, project_id, form_id):
        self.id = project_id
        self.form = form_id
        self.ops = f"/api/v1/projects/{project_id}/plugin-packages"
        self.contributions = f"/api/v1/projects/{project_id}/plugin-contributions"
        self.preferences = f"/api/v1/projects/{project_id}/plugin-preferences"

    def bind(self, key, package, generation, grants, token, placements=None, request_id=None, expect=0,
             reason=None, error_code=None):
        body = {"package_id": package, "expected_generation": generation, "grants": grants,
                "request_id": request_id or str(uuid4())}
        if placements is not None:
            body["placements"] = placements
        return request("POST", f"{self.ops}/{key}/binding", body, token, expect, reason, error_code)

    def discover(self, token, form=None, expect=0, reason=None):
        query = {"surface": "form", "form_id": form} if form else {"surface": "project"}
        data = request("GET", f"{self.contributions}?{urlencode(query)}", token=token, expect=expect, reason=reason)
        return data["items"] if data else data

    def entry(self, key, contribution, token, expect=0, reason=None, **context):
        query = urlencode(context)
        return request("GET", f"{self.contributions}/{key}/{contribution}/entry?{query}", token=token,
                       expect=expect, reason=reason)

    def data(self, key, contribution, item, token, op="form.describe", expect=0, reason=None, error_code=None,
             **extra):
        body = {"package_id": item["package_id"], "generation": item["generation"],
                "request_id": str(uuid4()), "op": op, "read": "orders", **extra}
        return request("POST", f"{self.contributions}/{key}/{contribution}/data", body, token, expect, reason,
                       error_code)

    def preference(self, key, contribution, token, hidden=False, collapsed=False, position=None, expect=0,
                   reason=None):
        return request("PUT", f"{self.preferences}/{key}/{contribution}",
                       {"hidden": hidden, "collapsed": collapsed, "position": position}, token, expect, reason)

    def binding(self, key, token):
        catalog = request("GET", self.ops, token=token)
        return next(row for row in catalog["bindings"] if row["key"] == key), catalog


def record_items(record, token, expect=0, reason=None):
    data = request("GET", f"/api/v1/form-records/{record}/plugin-contributions", token=token, expect=expect,
                   reason=reason)
    return data["items"] if data else data


def by_id(items, contribution):
    return next(item for item in items if item["contribution"]["id"] == contribution)


def ids(items):
    return [item["contribution"]["id"] for item in items]


def manifest_cases(p1, owner):
    def rejected(package, reason, wasm=False):
        request("POST", p1.ops, package, owner, expect=400, reason=reason)

    with_ui = ui_package()
    with_ui["manifest"]["ui"] = {"slot": "form.record.detail"}
    rejected(with_ui, "manifest_malformed")
    nine = ui_package()
    nine["manifest"]["contributions"] = [
        samples.contribution(f"c{index}", "panel", "form.record.detail", "Panel", "v", None, f"form_{index}")
        for index in range(9)
    ]
    rejected(nine, "contributions_invalid")
    views = ui_package()
    views["manifest"]["contributions"] = [
        samples.contribution(f"v{index}", "view", "project.view", "View", "v", f"r{index}", None)
        for index in range(3)
    ]
    rejected(views, "contributions_invalid")
    carrying = ui_package()
    carrying["wasm_base64"] = encode(b"\0asm")
    rejected(carrying, "profile_mismatch")
    reserved = ui_package(key="library")
    rejected(reserved, "manifest_field_invalid")
    wrong = ui_package()
    wrong["manifest"]["entry"]["sha256"] = "0" * 64
    request("POST", p1.ops, wrong, owner, expect=400, reason="asset_digest_mismatch",
            error_code="checksum_mismatch")
    assert request("GET", p1.ops, token=owner)["items"] == []
    check("X18 manifest rejections leave no installed version")


def run_main(context_path):
    owner_id, owner = create_account("ext-owner")
    member_id, member = create_account("ext-member", owner)
    _, outsider = create_account("ext-outsider", owner)
    workspace = request("POST", "/api/v1/workspaces", {"slug": f"ext-{uuid4().hex[:8]}", "name": "Ext"}, owner)["id"]
    request("POST", f"/api/v1/workspaces/{workspace}/members", {"user_id": member_id, "role": "member"}, owner)
    projects = f"/api/v1/workspaces/{workspace}/projects"
    p1_id = request("POST", projects, {"key": "EXTA", "name": "Extension A"}, owner)["id"]
    p2_id = request("POST", projects, {"key": "EXTB", "name": "Extension B"}, owner)["id"]
    p1 = Project(p1_id, create_form(p1_id, owner))
    p2 = Project(p2_id, create_form(p2_id, owner))
    owner_record = create_record(p1.form, owner)["id"]
    member_record = create_record(p1.form, member, quantity="3")["id"]
    p2_record = create_record(p2.form, owner)["id"]

    catalog = request("GET", p1.ops, token=owner)
    assert catalog["extension_supported"] is True
    manifest_cases(p1, owner)

    request("POST", p1.ops, ui_package(), member, expect=403)
    board = request("POST", p1.ops, ui_package(), owner)
    assert request("POST", p1.ops, ui_package(), owner)["id"] == board["id"]
    assert board["placements"] == [
        {"id": "board", "enabled": True, "position": 50},
        {"id": "list_tool", "enabled": True, "position": 50},
        {"id": "record_summary", "enabled": True, "position": 50},
    ]
    assert p1.discover(member) == []
    check("X14 identical reinstall returns the existing version; installed but unbound is invisible")

    # X1: only ui.display.
    bound = p1.bind("order_board", board["id"], 0, ["ui.display"], owner)
    assert bound["generation"] == 1 and bound["grants"] == ["ui.display"]
    view = p1.discover(member)
    assert ids(view) == ["board"] and view[0]["grants"] == ["ui.display"] and view[0]["profile"] == "ui"
    assert ids(p1.discover(member, form=p1.form)) == ["list_tool"]
    assert ids(record_items(owner_record, member)) == ["record_summary"]
    p1.data("order_board", "board", view[0], member, expect=403, reason="data_read_not_granted")
    check("X1 ui.display alone shows every slot and denies data reads")

    # X7: administration endpoints stay admin-only.
    p1.bind("order_board", board["id"], 1, ["ui.display"], member, expect=403)
    request("POST", f"{p1.ops}/order_board/configuration/reset",
            {"expected_generation": 1, "request_id": str(uuid4())}, member, expect=403)
    check("X7 members cannot bind or reset configuration")

    # X2: one read grant shared by every contribution.
    p1.bind("order_board", board["id"], 1, ["read:orders", "ui.display"], owner)
    view = p1.discover(member)[0]
    record_panel = by_id(record_items(owner_record, member), "record_summary")
    assert view["grants"] == record_panel["grants"] == ["read:orders", "ui.display"]
    stale = dict(view, generation=1)
    p1.data("order_board", "board", stale, member, expect=409, error_code="version_conflict")
    described = p1.data("order_board", "board", view, member)
    assert [field["key"] for field in described["form"]["fields"]] == ["quantity", "unit_price", "amount"]
    assert described["missing_fields"] == [] and described["denied_fields"] == []
    first = p1.data("order_board", "board", view, member, op="records.query", limit=1)
    assert len(first["items"]) == 1 and first["next_cursor"]
    assert set(first["items"][0]["values"]) <= {"quantity", "unit_price", "amount"}
    assert "title" not in first["items"][0]
    second = p1.data("order_board", "record_summary", record_panel, member, op="records.query", limit=1,
                     cursor=first["next_cursor"])
    assert len(second["items"]) == 1 and second["next_cursor"] is None
    assert {first["items"][0]["id"], second["items"][0]["id"]} == {owner_record, member_record}
    tampered = first["next_cursor"][:-2] + ("AA" if not first["next_cursor"].endswith("AA") else "BB")
    p1.data("order_board", "board", view, member, op="records.query", cursor=tampered, expect=400,
            reason="data_op_invalid")
    p1.data("order_board", "board", view, member, op="records.query", limit=51, expect=400,
            reason="data_op_invalid")
    p1.data("order_board", "board", view, member, op="records.delete", expect=400, reason="data_op_invalid")
    got = p1.data("order_board", "board", view, member, op="record.get", record_id=owner_record)
    assert got["item"]["id"] == owner_record
    assert got["item"]["values"]["quantity"] == {"type": "decimal", "decimal": "2"}
    p1.data("order_board", "board", view, member, op="record.get", record_id=p2_record, expect=404,
            reason="record_unavailable")
    check("X2 read grant shared by view and record panel; strict cursors and record scope")

    # Entry context rules.
    opened = p1.entry("order_board", "board", member)
    assert opened["item"]["contribution"]["id"] == "board" and "<" in opened["html"]
    assert opened["item"]["web_sha256"] == board["manifest"]["entry"]["sha256"]
    p1.entry("order_board", "board", member, expect=400, reason="context_invalid", form_id=p1.form)
    p1.entry("order_board", "record_summary", member, expect=400, reason="context_invalid")
    p1.entry("order_board", "record_summary", member, expect=400, reason="context_invalid", record_id=p2_record)
    p1.entry("order_board", "list_tool", member, expect=400, reason="context_invalid", record_id=owner_record)
    assert p1.entry("order_board", "list_tool", member, form_id=p1.form)["item"]["contribution"]["id"] == "list_tool"
    assert p1.entry("order_board", "record_summary", member, record_id=owner_record)["html"]
    request("GET", f"{p1.contributions}?surface=board", token=member, expect=400, reason="context_invalid")
    request("GET", f"{p1.contributions}?surface=project&form_id={p1.form}", token=member, expect=400,
            reason="context_invalid")
    request("GET", f"{p1.contributions}?surface=form", token=member, expect=400, reason="context_invalid")
    request("GET", f"{p1.contributions}?surface=form&form_id={p2.form}", token=member, expect=400,
            reason="context_invalid")
    check("entry and discovery context validation")

    # X8: another project or a non-member sees nothing.
    assert p2.discover(member) == []
    p2.entry("order_board", "board", member, expect=404, reason="contribution_unavailable")
    p2.data("order_board", "board", view, member, expect=404, reason="contribution_unavailable")
    assert record_items(p2_record, member) == []
    p1.entry("order_board", "board", outsider, expect=404, reason="contribution_unavailable")
    p1.data("order_board", "board", view, outsider, expect=404, reason="contribution_unavailable")
    request("GET", f"{p1.contributions}?surface=project", token=outsider, expect=404)
    check("X8 cross-project and non-member access is unavailable")

    # X11 + X9: concurrent layout writes on one generation, replay, conflicting reuse.
    layout = [{"id": "board", "enabled": False, "position": 50},
              {"id": "record_summary", "enabled": True, "position": 10},
              {"id": "list_tool", "enabled": True, "position": 50}]
    other = [{"id": "board", "enabled": True, "position": 20},
             {"id": "record_summary", "enabled": True, "position": 30},
             {"id": "list_tool", "enabled": False, "position": 50}]
    grants = ["read:orders", "ui.display"]
    winner_id, loser_id = str(uuid4()), str(uuid4())
    barrier = threading.Barrier(2)

    def contend(placements, request_id):
        barrier.wait()
        body = {"package_id": board["id"], "expected_generation": 2, "grants": grants,
                "request_id": request_id, "placements": placements}
        return call("POST", f"{p1.ops}/order_board/binding", body, owner)[0]

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(lambda args: contend(*args), [(layout, winner_id), (other, loser_id)]))
    codes = sorted(result["code"] for result in results)
    assert codes == [0, 409], results
    loser = next(result for result in results if result["code"] == 409)
    assert loser["error_code"] == "version_conflict"
    won = next(result for result in results if result["code"] == 0)["data"]
    if won["placements"] != sorted(layout, key=lambda row: row["id"]):
        # The other request won the race; apply the intended layout on top of it.
        layout_winner = loser_id if won["placements"] == sorted(other, key=lambda row: row["id"]) else None
        assert layout_winner is not None
        won = p1.bind("order_board", board["id"], won["generation"], grants, owner, layout, request_id=winner_id)
    replay = p1.bind("order_board", board["id"], won["generation"] - 1, grants, owner, list(reversed(layout)),
                     request_id=winner_id)
    assert replay == won
    p1.bind("order_board", board["id"], won["generation"] - 1, ["ui.display"], owner, layout,
            request_id=winner_id, expect=409)
    _, catalog = p1.binding("order_board", owner)
    assert [row["request_id"] for row in catalog["binding_history"]].count(winner_id) == 1
    assert all("reason" in row for row in catalog["binding_history"])
    check("X11 concurrent bind: one success, one version_conflict; replay identical; reuse conflicts")
    generation = won["generation"]

    assert p1.discover(member) == []
    p1.entry("order_board", "board", member, expect=404, reason="contribution_unavailable")
    p1.data("order_board", "board", dict(view, generation=generation), member, expect=404,
            reason="contribution_unavailable")
    assert by_id(record_items(owner_record, member), "record_summary")["project_position"] == 10
    check("X9 disabled view disappears and its direct link is unavailable; position persisted")

    # X10: personal preferences.
    p1.preference("order_board", "board", member, position=5, expect=404, reason="contribution_unavailable")
    enabled_layout = [dict(row, enabled=True) if row["id"] == "board" else row for row in layout]
    generation = p1.bind("order_board", board["id"], generation, grants, owner, enabled_layout)["generation"]
    request("PUT", f"{p1.preferences}/order_board/board", {"hidden": False, "collapsed": False}, member,
            expect=400, reason="preference_invalid")
    request("PUT", f"{p1.preferences}/order_board/board", {"hidden": False, "collapsed": False, "position": 100},
            member, expect=400, reason="preference_invalid")
    hidden = p1.preference("order_board", "list_tool", member, hidden=True)
    assert hidden["hidden"] is True and hidden["key"] == "order_board" and hidden["contribution_id"] == "list_tool"
    assert p1.preference("order_board", "list_tool", member, hidden=True)["hidden"] is True
    p1.preference("order_board", "record_summary", member, collapsed=True)
    p1.preference("order_board", "board", member, position=5)
    view = p1.discover(member)[0]
    assert view["effective_position"] == 5 and view["project_position"] == 50
    toolbar = p1.discover(member, form=p1.form)[0]
    assert toolbar["preference"]["hidden"] is True
    assert by_id(record_items(owner_record, member), "record_summary")["preference"]["collapsed"] is True
    owner_view = p1.discover(owner)[0]
    assert owner_view["effective_position"] == 50 and owner_view["preference"] == {
        "hidden": False, "collapsed": False, "position": None}
    assert binding_generation(p1, owner) == generation
    check("X10 preferences apply only to their owner and never change the generation")

    # X12: upgrade without placements keeps layout and preferences per contribution.
    upgrade = request("POST", p1.ops, ui_package("1.1.0"), owner)
    old_view = view
    upgraded = p1.bind("order_board", upgrade["id"], generation, grants, owner)
    generation = upgraded["generation"]
    assert upgraded["placements"] == [
        {"id": "board", "enabled": True, "position": 50},
        {"id": "record_action", "enabled": True, "position": 50},
        {"id": "record_summary", "enabled": True, "position": 10},
    ]
    assert p1.discover(member, form=p1.form) == []
    record_view = record_items(owner_record, member)
    assert ids(record_view) == ["record_summary", "record_action"]
    assert by_id(record_view, "record_summary")["preference"]["collapsed"] is True
    assert p1.discover(member)[0]["effective_position"] == 5
    check("X12 upgrade keeps layouts and preferences; new contribution uses defaults; removed one hidden")

    # X13: rollback restores the toolbar with its layout and hidden preference; old frames are stale.
    rolled = p1.bind("order_board", board["id"], generation, grants, owner)
    generation = rolled["generation"]
    assert [row for row in rolled["placements"] if row["id"] == "list_tool"] == [
        {"id": "list_tool", "enabled": True, "position": 50}]
    toolbar = p1.discover(member, form=p1.form)[0]
    assert toolbar["preference"]["hidden"] is True
    p1.data("order_board", "board", old_view, member, expect=409, error_code="version_conflict")
    check("X13 rollback restores layout rows and preferences; stale generation rejected")

    # X14: disable and re-enable the same package.
    disabled = p1.bind("order_board", None, generation, [], owner)
    generation = disabled["generation"]
    assert "placements" not in disabled
    assert p1.discover(member) == [] and record_items(owner_record, member) == []
    p1.entry("order_board", "board", member, expect=404, reason="contribution_unavailable")
    p1.data("order_board", "board", dict(view, generation=generation), member, expect=409,
            error_code="version_conflict")
    generation = p1.bind("order_board", board["id"], generation, grants, owner)["generation"]
    view = p1.discover(member)[0]
    assert view["effective_position"] == 5 and view["generation"] == generation
    check("X14 disable hides every entry; re-enable restores layout and preferences")

    # X15: revocation races. Form permission revocation against concurrent reads.
    policies = f"/api/v1/forms/{p1.form}/permissions"
    leaks = 0
    for _ in range(10):
        request("PATCH", policies, {"policies": [{"subject_type": "role", "subject_id": "member",
                                                  "policy": {"actions": {"form.view": True}}}]}, owner)
        leaks += race(p1, view, member, lambda: call("PATCH", policies, {"policies": [{
            "subject_type": "role", "subject_id": "member", "policy": {"actions": {"form.view": False}}}]}, owner))
    assert leaks == 0
    request("PATCH", policies, {"policies": [{"subject_type": "role", "subject_id": "member",
                                              "policy": {"actions": {"form.view": True}}}]}, owner)
    time.sleep(10)
    # Binding read-grant revocation against concurrent reads.
    for _ in range(10):
        current = binding_generation(p1, owner)
        if "read:orders" not in p1.discover(member)[0]["grants"]:
            current = p1.bind("order_board", board["id"], current, grants, owner)["generation"]
        item = p1.discover(member)[0]
        revoke_body = {"package_id": board["id"], "expected_generation": current, "grants": ["ui.display"],
                       "request_id": str(uuid4())}
        leaks += race(p1, item, member, lambda body=revoke_body: call(
            "POST", f"{p1.ops}/order_board/binding", body, owner))
    assert leaks == 0
    check("X15 20 reads raced against revocations; nothing returned after a revocation committed")
    time.sleep(10)

    generation = binding_generation(p1, owner)
    view = p1.discover(member)[0]
    assert view["grants"] == ["ui.display"]
    p1.data("order_board", "board", view, member, expect=403, reason="data_read_not_granted")
    generation = p1.bind("order_board", board["id"], generation, [], owner)["generation"]
    assert p1.discover(member) == []
    p1.entry("order_board", "board", member, expect=404, reason="contribution_unavailable")
    p1.preference("order_board", "board", member, position=7, expect=404, reason="contribution_unavailable")
    check("X15 revoking read and ui.display closes data, entry and preference writes")

    # X16: configuration reset.
    request("POST", f"{p1.ops}/order_board/configuration/reset",
            {"expected_generation": generation, "request_id": str(uuid4())}, owner, expect=400,
            reason="binding_active")
    generation = p1.bind("order_board", None, generation, [], owner)["generation"]
    reset_id = str(uuid4())
    reset = request("POST", f"{p1.ops}/order_board/configuration/reset",
                    {"expected_generation": generation, "request_id": reset_id}, owner)
    assert reset == {"key": "order_board", "package_id": None, "generation": generation + 1, "grants": []}
    assert request("POST", f"{p1.ops}/order_board/configuration/reset",
                   {"expected_generation": generation, "request_id": reset_id}, owner) == reset
    request("POST", f"{p1.ops}/order_board/configuration/reset",
            {"expected_generation": generation + 1, "request_id": reset_id}, owner, expect=409)
    generation = reset["generation"]
    _, catalog = p1.binding("order_board", owner)
    assert catalog["binding_history"][0]["reason"] == "configuration_reset"
    restored = p1.bind("order_board", board["id"], generation, grants, owner)
    generation = restored["generation"]
    assert all(row["position"] == 50 and row["enabled"] for row in restored["placements"])
    assert p1.discover(member)[0]["preference"] == {"hidden": False, "collapsed": False, "position": None}
    check("X16 reset refused while active; after disable it clears layouts and every preference")

    # Personal preference deletion endpoints.
    p1.preference("order_board", "board", member, position=3)
    p1.preference("order_board", "list_tool", member, hidden=True)
    assert request("DELETE", f"{p1.preferences}/order_board/board", token=member) == {"reset": True}
    assert request("DELETE", f"{p1.preferences}/order_board/board", token=member) == {"reset": True}
    assert request("DELETE", p1.preferences, token=member) == {"reset": 1}
    check("preference reset endpoints act only on the caller's rows")

    # X5: WASM profile through v3 invoke.
    wasm = samples.build_wasm()
    amount = request("POST", p1.ops, wasm_package(wasm), owner)
    amount_grants = ["amount.apply", "amount.preview", "ui.display"]
    amount_generation = p1.bind("order_amount_v3", amount["id"], 0, amount_grants, owner)["generation"]
    record_view = [item for item in record_items(owner_record, member) if item["key"] == "order_amount_v3"]
    assert [item["grants"] for item in record_view] == [amount_grants, amount_grants]
    amount_view = [item for item in p1.discover(member) if item["key"] == "order_amount_v3"]
    assert amount_view[0]["grants"] == ["ui.display"]
    invoke_path = f"/api/v1/form-records/{owner_record}/plugin-contributions/order_amount_v3"

    def invoke(contribution, capability, request_id=None, preview=None, expect=0, reason=None, token=member):
        body = {"package_id": amount["id"], "generation": amount_generation, "capability": capability,
                "input": {}, "request_id": request_id or str(uuid4())}
        if preview:
            body["preview_request_id"] = preview
        return request("POST", f"{invoke_path}/{contribution}/invoke", body, token, expect, reason)

    previewed = invoke("actions", "amount.preview")
    assert previewed["amount_cents"] == 2500
    assert invoke("actions", "amount.preview", previewed["request_id"]) == previewed
    invoke("detail", "amount.preview", previewed["request_id"], expect=400, reason="contribution_mismatch")
    invoke("view", "amount.preview", expect=404, reason="contribution_unavailable")
    applied = invoke("actions", "amount.apply", preview=previewed["request_id"])
    assert applied["amount_cents"] == 2500
    saved = request("GET", f"/api/v1/form-records/{owner_record}", token=owner)
    assert saved["values"]["amount"]["decimal"] == "25.00"
    receipts = request("GET", f"{p1.ops}/invocations", token=owner)
    assert {row["contribution_id"] for row in receipts if row["key"] == "order_amount_v3"} == {"actions"}
    check("X5 v3 WASM preview and apply with contribution receipts")

    # X6: restricted member policy (field denial and owned scope), then preview-only.
    request("PATCH", policies, {"policies": [{"subject_type": "role", "subject_id": "member", "policy": {
        "fields": {"unit_price": {"read": False}}, "record_scope": "owned"}}]}, owner)
    generation = binding_generation(p1, owner)
    generation = p1.bind("order_board", board["id"], generation, grants, owner)["generation"] \
        if "read:orders" not in p1.discover(member)[0]["grants"] else generation
    view = next(item for item in p1.discover(member) if item["key"] == "order_board")
    described = p1.data("order_board", "board", view, member)
    assert described["denied_fields"] == ["unit_price"]
    assert [field["key"] for field in described["form"]["fields"]] == ["quantity", "amount"]
    page = p1.data("order_board", "board", view, member, op="records.query")
    assert [row["id"] for row in page["items"]] == [member_record]
    assert "unit_price" not in page["items"][0]["values"]
    p1.data("order_board", "board", view, member, op="record.get", record_id=owner_record, expect=404,
            reason="record_unavailable")
    request("PATCH", policies, {"policies": [{"subject_type": "role", "subject_id": "member", "policy": {
        "actions": {"record.update": False}}}]}, owner)
    record_view = [item for item in record_items(owner_record, member) if item["key"] == "order_amount_v3"]
    assert record_view[0]["grants"] == ["amount.preview", "ui.display"]
    preview_only = invoke("actions", "amount.preview")
    invoke("actions", "amount.apply", preview=preview_only["request_id"], expect=403)
    request("PATCH", policies, {"policies": [{"subject_type": "role", "subject_id": "member", "policy": {}}]},
            owner)
    check("X6 denied field, owned scope and preview-only member")

    # X17: data rate limit.
    view = next(item for item in p1.discover(member) if item["key"] == "order_board")
    outcomes = [call("POST", f"{p1.contributions}/order_board/board/data", {
        "package_id": view["package_id"], "generation": view["generation"], "request_id": str(uuid4()),
        "op": "form.describe", "read": "orders"}, member)[0] for _ in range(32)]
    limited = [body for body in outcomes if body["code"] != 0]
    assert limited and all((body.get("details") or {}).get("reason") == "data_rate_limited" for body in limited)
    check("X17 data requests are rate limited per actor, project and key")
    time.sleep(10)

    # X15: library link revocation.
    library = request("POST", "/api/v1/me/form-plugin-packages", ui_package(key="library_board"), owner)
    request("POST", f"/api/v1/me/form-plugin-packages/{library['id']}/preview",
            {"quantity": 1, "unit_price_cents": 1}, owner, expect=400, reason="profile_mismatch")
    panel = request("GET", f"/api/v1/me/form-plugin-packages/{library['id']}/panel", token=owner)
    assert panel["web_sha256"] == library["manifest"]["entry"]["sha256"]
    attached = request("POST", f"{p1.ops}/library/{library['id']}", token=owner)
    p1.bind("library_board", attached["package"]["id"], 0, ["ui.display"], owner)
    assert "library_board" in [item["key"] for item in p1.discover(member)]
    request("DELETE", f"{p1.ops}/library/{library['id']}", token=owner)
    assert "library_board" not in [item["key"] for item in p1.discover(member)]
    p1.entry("library_board", "board", member, expect=404, reason="contribution_unavailable")
    check("X15 library link revocation hides the projected package")

    # X15: removing the member closes every direct path; preferences stay.
    p1.preference("order_board", "board", member, position=9)
    request("DELETE", f"/api/v1/workspaces/{workspace}/members/{member_id}", token=owner)
    p1.entry("order_board", "board", member, expect=404, reason="contribution_unavailable")
    p1.data("order_board", "board", view, member, expect=404, reason="contribution_unavailable")
    check("X15 removed member loses entry and data access")

    final_generation = binding_generation(p1, owner)
    context = {"base": BASE, "owner_token": owner, "owner_id": owner_id, "project_id": p1.id,
               "form_id": p1.form, "record_id": owner_record, "board_package": board["id"],
               "amount_package": amount["id"], "board_generation": final_generation,
               "amount_generation": amount_generation, "grants": grants}
    if context_path:
        context_path.write_text(json.dumps(context))
        context_path.chmod(0o600)
    return {"project_id": p1.id, "record_id": owner_record}


def binding_generation(project, token):
    return project.binding("order_board", token)[0]["generation"]


def race(project, item, token, revoke):
    """Runs one data read concurrently with one revocation and counts reads that returned data
    although they started after the revocation response had arrived."""
    revoked_at = []
    barrier = threading.Barrier(2)

    def read():
        barrier.wait()
        time.sleep(0.002)
        return call("POST", f"{project.contributions}/order_board/board/data", {
            "package_id": item["package_id"], "generation": item["generation"], "request_id": str(uuid4()),
            "op": "records.query", "read": "orders"}, token)

    def revoke_now():
        barrier.wait()
        body, _, finished = revoke()
        assert body["code"] == 0, body
        revoked_at.append(finished)

    with ThreadPoolExecutor(max_workers=2) as pool:
        reading = pool.submit(read)
        pool.submit(revoke_now).result()
        body, started, _ = reading.result()
    after = call("POST", f"{project.contributions}/order_board/board/data", {
        "package_id": item["package_id"], "generation": item["generation"], "request_id": str(uuid4()),
        "op": "records.query", "read": "orders"}, token)[0]
    leaked = int(after["code"] == 0)
    if body["code"] == 0 and started > revoked_at[0]:
        leaked += 1
    return leaked


def load(context_path):
    return json.loads(context_path.read_text())


def run_gate_off(context_path):
    context = load(context_path)
    owner, project = context["owner_token"], context["project_id"]
    ops = f"/api/v1/projects/{project}/plugin-packages"
    catalog = request("GET", ops, token=owner)
    assert catalog["extension_supported"] is False
    package = ui_package("2.0.0")
    request("POST", ops, package, owner, expect=403, reason="extensions_disabled")
    request("GET", f"/api/v1/projects/{project}/plugin-contributions?surface=project", token=owner, expect=403,
            reason="extensions_disabled")
    request("GET", f"/api/v1/form-records/{context['record_id']}/plugin-contributions", token=owner,
            expect=403, reason="extensions_disabled")
    request("GET", f"/api/v1/projects/{project}/plugin-contributions/order_board/board/entry", token=owner,
            expect=403, reason="extensions_disabled")
    amount = next(row for row in catalog["bindings"] if row["key"] == "order_amount_v3")
    request("POST", f"{ops}/order_amount_v3/binding", {
        "package_id": amount["package_id"], "expected_generation": amount["generation"],
        "grants": ["ui.display"], "request_id": str(uuid4())}, owner, expect=403, reason="extensions_disabled")
    disabled = request("POST", f"{ops}/order_amount_v3/binding", {
        "package_id": None, "expected_generation": amount["generation"], "grants": [],
        "request_id": str(uuid4())}, owner)
    context["amount_generation"] = disabled["generation"]
    context_path.write_text(json.dumps(context))
    check("X21 switch off: v3 install, discovery, entry and binding refused; disable allowed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", default=BASE)
    parser.add_argument("--context", type=Path, help="private file shared between phases")
    parser.add_argument("--phase", choices=["main", "gate-off"], default="main")
    arguments = parser.parse_args()
    BASE = arguments.base
    summary = {"main": run_main, "gate-off": run_gate_off}[arguments.phase](arguments.context) or {}
    print(json.dumps({"base_url": BASE, "phase": arguments.phase, **summary, "verified": VERIFIED}, indent=2))
