#!/usr/bin/env python3
"""Flow performance measurements against a release API and PostgreSQL.

The GitHub Actions workflow starts the processes. This script only drives HTTP,
a second-process WebSocket, and EXPLAIN (ANALYZE). It does not build the server.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

BLOCKS_PER_BATCH = 50
OPS_PER_BLOCK = 2
BATCH_OPS = BLOCKS_PER_BATCH * OPS_PER_BLOCK
COMMENT_COUNT = 20
BOOTSTRAP_SAMPLES = 5
LOGIN_SAMPLES = 5
CREATE_SAMPLES = 5
SEARCH_SAMPLES = 5
LIST_SAMPLES = 5
FANOUT_SAMPLES = 8


def eprint(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def percentile(samples: list[float], percent: int) -> float | None:
    if not samples:
        return None
    ordered = sorted(samples)
    last = len(ordered) - 1
    return ordered[(last * percent) // 100]


def stats(samples: list[float]) -> dict:
    if not samples:
        return {"n": 0}
    return {
        "n": len(samples),
        "min_ms": min(samples),
        "p50_ms": percentile(samples, 50),
        "p95_ms": percentile(samples, 95),
        "max_ms": max(samples),
        "mean_ms": statistics.fmean(samples),
        "samples_ms": samples,
    }


def write_part(out: Path, name: str, payload: dict) -> None:
    parts = out / "parts"
    parts.mkdir(parents=True, exist_ok=True)
    target = parts / f"{name}.json"
    target.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    eprint(f"wrote {target}")


def load_ids(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def save_ids(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    path.chmod(0o600)


class ApiError(RuntimeError):
    pass


def call(method: str, url: str, token: str | None, payload: dict | None, timeout: float) -> tuple[dict, float, int]:
    data = None if payload is None else json.dumps(payload).encode()
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            status = response.status
    except urllib.error.HTTPError as err:
        body = err.read()
        status = err.code
    elapsed = (time.perf_counter() - started) * 1000.0
    try:
        parsed = json.loads(body)
    except json.JSONDecodeError:
        parsed = None
    if status >= 400 or not isinstance(parsed, dict) or parsed.get("code") != 0:
        snippet = body[:600].decode("utf-8", "replace")
        raise ApiError(f"{method} {urllib.parse.urlparse(url).path} HTTP {status} in {elapsed:.1f} ms: {snippet}")
    data_field = parsed.get("data")
    if not isinstance(data_field, dict):
        raise ApiError(f"{method} {url} returned no data object")
    return data_field, elapsed, len(body)


def new_key() -> str:
    return str(uuid.uuid4())


def create_page(origin: str, token: str, workspace_id: str, title: str) -> tuple[dict, float]:
    data, elapsed, _size = call(
        "POST",
        f"{origin}/api/v1/workspaces/{workspace_id}/flow/objects",
        token,
        {"object_type": "page", "title": title, "idempotency_key": new_key()},
        60,
    )
    obj = data.get("object")
    if not isinstance(obj, dict) or "id" not in obj or "document_id" not in obj:
        raise ApiError(f"create page response missing object ids: {list(data)[:12]}")
    return {"id": obj["id"], "document_id": obj["document_id"], "title": title}, elapsed


def semantic_batches(origin: str, token: str, object_id: str, blocks: int, token_word: str) -> dict:
    if blocks % BLOCKS_PER_BATCH != 0:
        raise ApiError(f"block count {blocks} is not a multiple of {BLOCKS_PER_BATCH}")
    samples: list[float] = []
    failures: list[str] = []
    started = time.perf_counter()
    for batch_index, start in enumerate(range(0, blocks, BLOCKS_PER_BATCH)):
        operations = []
        for offset in range(BLOCKS_PER_BATCH):
            index = start + offset
            block_id = f"b{index:05d}"
            text = f"benchblock {index} alpha"
            if index == blocks - 1:
                text = f"benchblock {index} {token_word}"
            operations.append(
                {"op": "create_node", "id": block_id, "parent": None, "index": 0, "kind": "block"}
            )
            operations.append({"op": "insert_text", "id": block_id, "index": 0, "text": text})
        if len(operations) != BATCH_OPS:
            raise ApiError(f"batch built {len(operations)} operations, expected {BATCH_OPS}")
        try:
            _data, elapsed, _size = call(
                "POST",
                f"{origin}/api/v1/flow/objects/{object_id}/commands",
                token,
                {
                    "command": {"type": "semantic_patch", "payload": {"operations": operations}},
                    "idempotency_key": new_key(),
                },
                180,
            )
            samples.append(elapsed)
        except ApiError as err:
            failures.append(f"batch {batch_index} at block {start}: {err}")
            break
        if batch_index % 10 == 0:
            eprint(f"  semantic_patch {token_word} batch {batch_index + 1} {elapsed:.0f} ms")
    return {
        "blocks": blocks,
        "blocks_per_batch": BLOCKS_PER_BATCH,
        "operations_per_batch": BATCH_OPS,
        "batches_completed": len(samples),
        "batches_expected": blocks // BLOCKS_PER_BATCH,
        "wall_ms": (time.perf_counter() - started) * 1000.0,
        "batch": stats(samples),
        "failures": failures,
        "ok": len(samples) == blocks // BLOCKS_PER_BATCH and not failures,
    }


def steady_patch(origin: str, token: str, object_id: str) -> dict:
    operations = [
        {"op": "create_node", "id": "steady-extra", "parent": None, "index": 0, "kind": "block"},
        {"op": "insert_text", "id": "steady-extra", "index": 0, "text": "benchblock steady extra"},
    ]
    data, elapsed, _size = call(
        "POST",
        f"{origin}/api/v1/flow/objects/{object_id}/commands",
        token,
        {
            "command": {"type": "semantic_patch", "payload": {"operations": operations}},
            "idempotency_key": new_key(),
        },
        180,
    )
    return {"ms": elapsed, "accepted_seq": data.get("accepted_seq"), "ok": True}


def bootstrap_once(origin: str, token: str, object_id: str) -> tuple[dict, float, int]:
    data, elapsed, size = call(
        "GET",
        f"{origin}/api/v1/flow/objects/{object_id}/bootstrap",
        token,
        None,
        180,
    )
    snapshot = data.get("snapshot_base64")
    tail = data.get("tail_updates")
    tail_len = len(tail) if isinstance(tail, list) else None
    summary = {
        "document_id": data.get("document_id"),
        "snapshot_seq": data.get("snapshot_seq"),
        "head_seq": data.get("head_seq"),
        "snapshot_base64_chars": len(snapshot) if isinstance(snapshot, str) else None,
        "tail_updates": tail_len,
        "response_bytes": size,
    }
    return summary, elapsed, size


def search_once(origin: str, token: str, workspace_id: str, query: str) -> tuple[dict, float, int]:
    params = urllib.parse.urlencode(
        {
            "q": query,
            "unprojected": "true",
            "object_type": "page",
            "limit": "10",
            "freshness": "allow_stale",
        }
    )
    data, elapsed, size = call(
        "GET",
        f"{origin}/api/v1/workspaces/{workspace_id}/flow/search?{params}",
        token,
        None,
        180,
    )
    items = data.get("items") if isinstance(data.get("items"), list) else []
    frontier = data.get("index_frontier") if isinstance(data.get("index_frontier"), dict) else {}
    hit_ids = []
    for item in items:
        if isinstance(item, dict) and isinstance(item.get("object"), dict):
            hit_ids.append(item["object"].get("id"))
    return (
        {
            "hits": len(items),
            "hit_ids": hit_ids,
            "index_lag": frontier.get("lag"),
            "index_stale": frontier.get("stale"),
            "response_bytes": size,
        },
        elapsed,
        size,
    )


def wait_for_search(origin: str, token: str, workspace_id: str, query: str, object_id: str) -> dict:
    started = time.perf_counter()
    deadline = started + 180
    last = None
    while time.perf_counter() < deadline:
        try:
            summary, elapsed, _size = search_once(origin, token, workspace_id, query)
        except ApiError as err:
            last = {"error": str(err)}
            time.sleep(2)
            continue
        last = summary
        last["sample_ms"] = elapsed
        if object_id in summary["hit_ids"]:
            last["wait_ms"] = (time.perf_counter() - started) * 1000.0
            last["ok"] = True
            return last
        time.sleep(2)
    return {
        "ok": False,
        "wait_ms": (time.perf_counter() - started) * 1000.0,
        "last": last,
        "error": f"search did not return {object_id} for {query!r} within 180s",
    }


def command(origin: str, token: str, object_id: str, command_type: str, payload: dict, timeout: float = 120) -> tuple[dict, float]:
    data, elapsed, _size = call(
        "POST",
        f"{origin}/api/v1/flow/objects/{object_id}/commands",
        token,
        {"command": {"type": command_type, "payload": payload}, "idempotency_key": new_key()},
        timeout,
    )
    return data, elapsed


def run_api(ids_path: Path, out: Path) -> None:
    origin = os.environ["FLOW_BENCH_API_ORIGIN"]
    email = f"flow-bench-{uuid.uuid4().hex[:8]}@bench.sylvode.test"
    password = f"Aa1!{uuid.uuid4().hex}Z"
    register_payload = {"email": email, "password": password, "name": "Flow Bench"}
    registered, register_ms, _size = call(
        "POST", f"{origin}/api/v1/auth/register", None, register_payload, 60
    )
    token = registered["tokens"]["access_token"]
    refresh = registered["tokens"]["refresh_token"]
    user = registered["user"]
    login_samples = []
    for _ in range(LOGIN_SAMPLES):
        _data, elapsed, _size = call(
            "POST",
            f"{origin}/api/v1/auth/login",
            None,
            {"email": email, "password": password},
            60,
        )
        login_samples.append(elapsed)
    workspace, workspace_ms, _size = call(
        "POST",
        f"{origin}/api/v1/workspaces",
        token,
        {"slug": f"fb-{uuid.uuid4().hex[:8]}", "name": "Flow Bench"},
        60,
    )
    workspace_id = workspace["id"]
    _feature, feature_ms, _size = call(
        "PUT",
        f"{origin}/api/v1/workspaces/{workspace_id}/features/flow",
        token,
        {"enabled": True, "default_member_level": "edit", "idempotency_key": new_key()},
        60,
    )
    create_samples = []
    created_pages = []
    for index in range(CREATE_SAMPLES):
        page, elapsed = create_page(origin, token, workspace_id, f"Empty {index}")
        create_samples.append(elapsed)
        created_pages.append(page)
    parent, parent_ms = create_page(origin, token, workspace_id, "Move target")
    small, small_ms = create_page(origin, token, workspace_id, "Small page")
    fanout, fanout_ms = create_page(origin, token, workspace_id, "Fanout page")
    page_1k, create_1k_ms = create_page(origin, token, workspace_id, "Bench 1k")
    page_5k, create_5k_ms = create_page(origin, token, workspace_id, "Bench 5k")
    eprint("seeding 1k blocks")
    seed_1k = semantic_batches(origin, token, page_1k["id"], 1000, "flowbench1k")
    eprint("seeding 5k blocks")
    seed_5k = semantic_batches(origin, token, page_5k["id"], 5000, "flowbench5k")
    steady = None
    steady_error = None
    if seed_5k["ok"]:
        try:
            steady = steady_patch(origin, token, page_5k["id"])
        except ApiError as err:
            steady_error = str(err)

    def sample_bootstrap(page: dict) -> dict:
        samples = []
        sizes = []
        meta = None
        error = None
        for _ in range(BOOTSTRAP_SAMPLES):
            try:
                meta, elapsed, size = bootstrap_once(origin, token, page["id"])
            except ApiError as err:
                error = str(err)
                break
            samples.append(elapsed)
            sizes.append(size)
        return {
            "object_id": page["id"],
            "document_id": page["document_id"],
            "ok": error is None and len(samples) == BOOTSTRAP_SAMPLES,
            "error": error,
            "latency": stats(samples),
            "response_bytes": stats([float(size) for size in sizes]),
            "last": meta,
        }

    boot_1k = sample_bootstrap(page_1k) if seed_1k["ok"] else {"ok": False, "error": "seed failed"}
    boot_5k = sample_bootstrap(page_5k) if seed_5k["ok"] else {"ok": False, "error": "seed failed"}

    search_ready = wait_for_search(origin, token, workspace_id, "flowbench5k", page_5k["id"])
    search_unique = []
    search_common = []
    search_error = None
    if search_ready.get("ok"):
        try:
            for _ in range(SEARCH_SAMPLES):
                summary, elapsed, _size = search_once(origin, token, workspace_id, "flowbench5k")
                summary["ms"] = elapsed
                search_unique.append(summary)
            for _ in range(SEARCH_SAMPLES):
                summary, elapsed, _size = search_once(origin, token, workspace_id, "benchblock")
                summary["ms"] = elapsed
                search_common.append(summary)
        except ApiError as err:
            search_error = str(err)
    else:
        search_error = search_ready.get("error")

    comment_samples = []
    comment_error = None
    try:
        for index in range(COMMENT_COUNT):
            _data, elapsed, _size = call(
                "POST",
                f"{origin}/api/v1/flow/objects/{page_1k['id']}/comments",
                token,
                {"body": f"bench comment {index}"},
                60,
            )
            comment_samples.append(elapsed)
    except ApiError as err:
        comment_error = str(err)
    list_samples = []
    list_error = None
    list_count = None
    if comment_error is None:
        for _ in range(LIST_SAMPLES):
            try:
                data, elapsed, _size = call(
                    "GET",
                    f"{origin}/api/v1/flow/objects/{page_1k['id']}/comments",
                    token,
                    None,
                    60,
                )
            except ApiError as err:
                list_error = str(err)
                break
            items = data.get("items") if isinstance(data.get("items"), list) else []
            list_count = len(items)
            list_samples.append(elapsed)

    move_small = None
    move_small_error = None
    move_1k = None
    move_1k_error = None
    try:
        _data, elapsed = command(
            origin, token, small["id"], "move_object", {"target_object_id": parent["id"]}
        )
        move_small = elapsed
    except ApiError as err:
        move_small_error = str(err)
    try:
        _data, elapsed = command(
            origin, token, page_1k["id"], "move_object", {"target_object_id": parent["id"]}, timeout=180
        )
        move_1k = elapsed
    except ApiError as err:
        move_1k_error = str(err)

    ids = {
        "api_origin": origin,
        "observer_origin": os.environ.get("FLOW_BENCH_OBSERVER_ORIGIN", ""),
        "web_origin": os.environ.get("FLOW_BENCH_WEB_ORIGIN", ""),
        "collab_origin": os.environ.get("FLOW_BENCH_COLLAB_ORIGIN", ""),
        "database_url": os.environ["FLOW_BENCH_DATABASE_URL"],
        "token": token,
        "refresh_token": refresh,
        "user": user,
        "workspace_id": workspace_id,
        "pages": {
            "parent": parent,
            "small": small,
            "fanout": fanout,
            "k1": page_1k,
            "k5": page_5k,
            "empty": created_pages,
        },
        "bootstrap_5k": boot_5k.get("last"),
    }
    save_ids(ids_path, ids)
    payload = {
        "ok": bool(seed_1k["ok"] and seed_5k["ok"] and boot_1k.get("ok") and boot_5k.get("ok")),
        "register_ms": register_ms,
        "login": stats(login_samples),
        "create_workspace_ms": workspace_ms,
        "enable_flow_ms": feature_ms,
        "create_page": stats(create_samples),
        "create_parent_ms": parent_ms,
        "create_small_ms": small_ms,
        "create_fanout_page_ms": fanout_ms,
        "create_1k_page_ms": create_1k_ms,
        "create_5k_page_ms": create_5k_ms,
        "semantic_patch_1k": seed_1k,
        "semantic_patch_5k": seed_5k,
        "semantic_patch_steady_on_5k": steady,
        "semantic_patch_steady_error": steady_error,
        "bootstrap_1k": boot_1k,
        "bootstrap_5k": boot_5k,
        "search_index_wait": {key: value for key, value in search_ready.items() if key != "hit_ids"},
        "search_unique": {
            "query": "flowbench5k",
            "ok": search_error is None and len(search_unique) == SEARCH_SAMPLES,
            "error": search_error,
            "latency": stats([item["ms"] for item in search_unique]),
            "last": search_unique[-1] if search_unique else None,
        },
        "search_common": {
            "query": "benchblock",
            "ok": search_error is None and len(search_common) == SEARCH_SAMPLES,
            "error": search_error,
            "latency": stats([item["ms"] for item in search_common]),
            "last": search_common[-1] if search_common else None,
        },
        "comment_create": {
            "ok": comment_error is None and len(comment_samples) == COMMENT_COUNT,
            "error": comment_error,
            "count": len(comment_samples),
            "latency": stats(comment_samples),
        },
        "comments_list": {
            "ok": list_error is None and len(list_samples) == LIST_SAMPLES,
            "error": list_error,
            "items": list_count,
            "latency": stats(list_samples),
        },
        "move_small_ms": move_small,
        "move_small_error": move_small_error,
        "move_1k_ms": move_1k,
        "move_1k_error": move_1k_error,
    }
    write_part(out, "api", payload)


def issue_ticket(origin: str, token: str, workspace_id: str, document_id: str, client_id: str, collab_origin: str) -> str:
    data, _elapsed, _size = call(
        "POST",
        f"{origin}/api/v1/collab/tickets",
        token,
        {
            "workspace_id": workspace_id,
            "document_id": document_id,
            "client_id": client_id,
            "origin": collab_origin,
        },
        30,
    )
    ticket = data.get("ticket")
    if not isinstance(ticket, str) or not ticket:
        raise ApiError("ticket response did not include a ticket")
    return ticket


async def fanout_session(ids: dict) -> dict:
    import websockets

    observer = ids["observer_origin"]
    token = ids["token"]
    document_id = ids["pages"]["fanout"]["document_id"]
    object_id = ids["pages"]["fanout"]["id"]
    client_id = "bench-fanout"
    collab_origin = ids["collab_origin"]
    ticket = issue_ticket(observer, token, ids["workspace_id"], document_id, client_id, collab_origin)
    query = urllib.parse.urlencode({"ticket": ticket, "client_id": client_id})
    ws_url = observer.replace("http://", "ws://", 1) + f"/api/v1/collab/ws?{query}"
    arrivals: dict[int, float] = {}
    errors: list[str] = []

    # websockets 13 names this extra_headers. Passing additional_headers falls through to
    # asyncio.create_connection and raises TypeError.
    async with websockets.connect(
        ws_url,
        extra_headers={"Origin": collab_origin},
        max_size=16 * 1024 * 1024,
        open_timeout=20,
    ) as ws:
        await ws.send(
            json.dumps(
                {
                    "type": "hello",
                    "protocol_version": 1,
                    "capabilities": [],
                    "client_id": client_id,
                    "session_id": str(uuid.uuid4()),
                }
            )
        )
        hello = json.loads(await asyncio.wait_for(ws.recv(), 15))
        if hello.get("type") != "hello":
            raise ApiError(f"expected hello, got {hello.get('type')}")
        await ws.send(
            json.dumps(
                {
                    "type": "open",
                    "protocol_version": 1,
                    "document_id": document_id,
                    "known_seq": None,
                    "known_frontier": None,
                }
            )
        )
        snapshot = None
        deadline = time.perf_counter() + 20
        while snapshot is None and time.perf_counter() < deadline:
            frame = json.loads(await asyncio.wait_for(ws.recv(), 20))
            if frame.get("type") == "snapshot":
                snapshot = frame
            elif frame.get("type") == "rejected":
                raise ApiError(f"open rejected: {frame.get('code')} {frame.get('details')}")
        if snapshot is None:
            raise ApiError("snapshot frame did not arrive")

        async def reader() -> None:
            try:
                async for raw in ws:
                    arrived = time.perf_counter()
                    frame = json.loads(raw)
                    kind = frame.get("type")
                    if kind == "ping":
                        await ws.send(
                            json.dumps(
                                {
                                    "type": "pong",
                                    "protocol_version": 1,
                                    "nonce": frame.get("nonce", ""),
                                }
                            )
                        )
                    elif kind == "accepted" and isinstance(frame.get("head_seq"), int):
                        arrivals[frame["head_seq"]] = arrived
                    elif kind == "rejected":
                        errors.append(json.dumps(frame)[:400])
            except asyncio.CancelledError:
                raise
            except Exception as err:  # noqa: BLE001 - the sample records the socket failure
                errors.append(str(err))

        reader_task = asyncio.create_task(reader())
        samples = []
        try:
            for index in range(FANOUT_SAMPLES + 1):
                started = time.perf_counter()
                data, http_ms = await asyncio.to_thread(
                    command,
                    ids["api_origin"],
                    token,
                    object_id,
                    "insert_block",
                    {"block_id": f"fan-{index}", "text": f"fanout sample {index}"},
                )
                http_done = time.perf_counter()
                accepted_seq = data.get("accepted_seq")
                if not isinstance(accepted_seq, int):
                    raise ApiError(f"insert_block returned no accepted_seq: {list(data)[:12]}")
                wait_deadline = time.perf_counter() + 15
                while accepted_seq not in arrivals and time.perf_counter() < wait_deadline:
                    await asyncio.sleep(0.002)
                if accepted_seq not in arrivals:
                    raise ApiError(
                        f"observer did not accept seq {accepted_seq}; arrivals={sorted(arrivals)} errors={errors[:3]}"
                    )
                fanout_ms = (arrivals[accepted_seq] - started) * 1000.0
                after_http_ms = (arrivals[accepted_seq] - http_done) * 1000.0
                sample = {
                    "accepted_seq": accepted_seq,
                    "http_ms": http_ms,
                    "fanout_ms": fanout_ms,
                    "after_http_response_ms": after_http_ms,
                    "warmup": index == 0,
                }
                if index != 0:
                    samples.append(sample)
                eprint(
                    f"  fanout sample {index} http {http_ms:.1f} ms end-to-end {fanout_ms:.1f} ms "
                    f"after response {after_http_ms:.1f} ms"
                )
        finally:
            reader_task.cancel()
            try:
                await reader_task
            except asyncio.CancelledError:
                pass
    return {
        "ok": len(samples) == FANOUT_SAMPLES,
        "snapshot_head_seq": snapshot.get("head_seq"),
        "samples": samples,
        "fanout_ms": stats([sample["fanout_ms"] for sample in samples]),
        "after_http_response_ms": stats([sample["after_http_response_ms"] for sample in samples]),
        "http_ms": stats([sample["http_ms"] for sample in samples]),
        "errors": errors,
        "note": "Writer is API process A. Observer is API process B, subscribed over the collab WebSocket. "
        "fanout_ms starts just before the REST insert_block and ends when B delivers the accepted frame. "
        "The first insert is a warmup and is omitted.",
    }


def run_fanout(ids_path: Path, out: Path) -> None:
    ids = load_ids(ids_path)
    try:
        payload = asyncio.run(fanout_session(ids))
    except Exception as err:  # noqa: BLE001 - recorded in the artifact
        payload = {"ok": False, "error": str(err)}
        eprint(f"fanout failed: {err}")
    write_part(out, "fanout", payload)


def psql_explain(database_url: str, sql: str, variables: dict[str, str]) -> dict:
    # psql does not interpolate :'variables' inside -c. -f - does.
    command = ["psql", database_url, "-X", "-q", "-t", "-A", "-v", "ON_ERROR_STOP=1", "-f", "-"]
    for key, value in variables.items():
        command.extend(["-v", f"{key}={value}"])
    completed = subprocess.run(command, check=False, capture_output=True, text=True, input=sql)
    if completed.returncode != 0:
        return {"ok": False, "error": (completed.stderr or completed.stdout)[-800:]}
    try:
        parsed = json.loads(completed.stdout)
    except json.JSONDecodeError as err:
        return {"ok": False, "error": f"explain output was not JSON: {err}"}
    if not isinstance(parsed, list) or not parsed or not isinstance(parsed[0], dict):
        return {"ok": False, "error": "explain JSON had an unexpected shape"}
    row = parsed[0]
    plan = row.get("Plan") if isinstance(row.get("Plan"), dict) else {}
    return {
        "ok": True,
        "planning_ms": row.get("Planning Time"),
        "execution_ms": row.get("Execution Time"),
        "node_type": plan.get("Node Type"),
        "actual_rows": plan.get("Actual Rows"),
        "shared_hit_blocks": plan.get("Shared Hit Blocks"),
        "shared_read_blocks": plan.get("Shared Read Blocks"),
        "plan": plan,
    }


def run_explain(ids_path: Path, out: Path) -> None:
    ids = load_ids(ids_path)
    database_url = ids["database_url"]
    page = ids["pages"]["k5"]
    boot = ids.get("bootstrap_5k") or {}
    snapshot_seq = boot.get("snapshot_seq")
    head_seq = boot.get("head_seq")
    document_id = page["document_id"]
    object_id = page["id"]
    workspace_id = ids["workspace_id"]
    queries: list[tuple[str, str, dict[str, str]]] = [
        (
            "object_view_5k",
            """
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT fo.id, fo.workspace_id, fo.project_id, fo.parent_id, fo.object_type,
                   fo.lifecycle_status, fo.governance_metadata, fo.created_at, fo.updated_at,
                   fo.archived_at, cd.id AS document_id, cd.head_seq AS document_seq,
                   cd.head_frontier AS document_frontier, p.title AS projection_title,
                   p.state AS projection_state, p.document_seq AS projection_document_seq
              FROM flow_objects fo
              INNER JOIN collab_documents cd ON cd.object_id = fo.id
              INNER JOIN flow_object_projections p ON p.object_id = fo.id
             WHERE fo.id = :'object_id'::uuid
            """,
            {"object_id": object_id},
        ),
        (
            "bootstrap_document_5k",
            """
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT fo.workspace_id, cd.engine, cd.format_version, cd.snapshot, cd.snapshot_checksum,
                   cd.snapshot_frontier, cd.snapshot_seq, cd.head_frontier, cd.head_seq
              FROM collab_documents cd
              JOIN flow_objects fo ON fo.id = cd.object_id
             WHERE cd.id = :'document_id'::uuid
            """,
            {"document_id": document_id},
        ),
        (
            "comments_list_1k_page",
            """
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT id, object_id, block_id, parent_id, author_id, body, mentions,
                   created_at, updated_at, resolved_at, resolved_by
              FROM flow_comments
             WHERE object_id = :'object_id'::uuid
               AND deleted_at IS NULL
               AND (parent_id IS NULL OR parent_id IN (
                    SELECT id FROM flow_comments
                     WHERE object_id = :'object_id'::uuid
                       AND deleted_at IS NULL
                       AND parent_id IS NULL))
             ORDER BY created_at ASC, id ASC
             LIMIT 501
            """,
            {"object_id": ids["pages"]["k1"]["id"]},
        ),
        (
            "search_benchblock",
            """
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            WITH search_query AS (
                SELECT websearch_to_tsquery('simple', :'query') AS terms
            ), ranked AS (
                SELECT fo.id AS object_id, fo.project_id, fo.object_type, fo.lifecycle_status,
                       si.title, si.indexed_seq, cd.head_seq,
                       p.document_seq AS projection_seq,
                       p.document_frontier AS projection_frontier,
                       si.indexed_frontier,
                       p.title AS projection_title,
                       p.plain_text AS projection_plain_text,
                       si.plain_text AS indexed_plain_text,
                       to_tsvector('simple', si.title) @@ sq.terms AS title_matches,
                       to_tsvector('simple', si.plain_text) @@ sq.terms AS body_matches,
                       ts_headline(
                           'simple', replace(replace(replace(si.title, '&', '&amp;'), '<', '&lt;'), '>', '&gt;'), sq.terms,
                           'StartSel=<mark>, StopSel=</mark>, MaxFragments=1, MinWords=1, MaxWords=12'
                       ) AS title_snippet,
                       ts_headline(
                           'simple', replace(replace(replace(si.plain_text, '&', '&amp;'), '<', '&lt;'), '>', '&gt;'), sq.terms,
                           'StartSel=<mark>, StopSel=</mark>, MaxFragments=2, MinWords=3, MaxWords=24'
                       ) AS body_snippet,
                       ts_rank_cd(si.search_vector, sq.terms) AS rank
                  FROM flow_search_index si
                  JOIN flow_objects fo ON fo.id = si.object_id
                  JOIN collab_documents cd ON cd.object_id = fo.id
                  JOIN flow_object_projections p ON p.object_id = fo.id
                 CROSS JOIN search_query sq
                 WHERE fo.workspace_id = :'workspace_id'::uuid
                   AND fo.lifecycle_status = 'active'
                   AND NOT flow_is_system_navigator_root(fo.object_type, fo.parent_id, fo.governance_metadata)
                   AND fo.project_id IS NULL
                   AND fo.object_type = 'page'
                   AND si.search_vector @@ sq.terms
            )
            SELECT * FROM ranked
            ORDER BY rank DESC, object_id ASC
            LIMIT 100
            """,
            {"query": "benchblock", "workspace_id": workspace_id},
        ),
        (
            "fanout_poll",
            """
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT id, document_id, document_seq
              FROM flow_fanout_notices
             WHERE id > 0 AND notice_kind = 'document_update'
             ORDER BY id ASC
             LIMIT 256
            """,
            {},
        ),
    ]
    if isinstance(snapshot_seq, int) and isinstance(head_seq, int):
        queries.insert(
            2,
            (
                "bootstrap_tail_5k",
                """
                EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
                SELECT seq, update_id, bytes, before_frontier, after_frontier, content_hash
                  FROM collab_updates
                 WHERE document_id = :'document_id'::uuid
                   AND seq > :'snapshot_seq'::bigint
                   AND seq <= :'head_seq'::bigint
                 ORDER BY seq ASC
                """,
                {
                    "document_id": document_id,
                    "snapshot_seq": str(snapshot_seq),
                    "head_seq": str(head_seq),
                },
            ),
        )
    results = {}
    for name, sql, variables in queries:
        eprint(f"explain {name}")
        results[name] = psql_explain(database_url, sql, variables)
    version = subprocess.run(
        ["psql", database_url, "-X", "-q", "-t", "-A", "-c", "SELECT version()"],
        check=False,
        capture_output=True,
        text=True,
    )
    payload = {
        "ok": all(item.get("ok") for item in results.values()) and bool(results),
        "postgres": version.stdout.strip() if version.returncode == 0 else version.stderr[-400:],
        "queries": results,
    }
    write_part(out, "explain", payload)


def load_part(out: Path, name: str) -> dict | None:
    path = out / "parts" / f"{name}.json"
    if not path.is_file():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def fmt_ms(value) -> str:
    if isinstance(value, (int, float)):
        return f"{value:.1f}"
    return "—"


def fmt_stats(block: dict | None) -> str:
    if not isinstance(block, dict) or not block.get("n"):
        return "—"
    return (
        f"n={block['n']} p50={fmt_ms(block.get('p50_ms'))} "
        f"p95={fmt_ms(block.get('p95_ms'))} min={fmt_ms(block.get('min_ms'))} "
        f"max={fmt_ms(block.get('max_ms'))}"
    )


def row(cells: list[str]) -> str:
    return "| " + " | ".join(cells) + " |"


def finalize(out: Path) -> int:
    api = load_part(out, "api")
    fanout = load_part(out, "fanout")
    explain = load_part(out, "explain")
    loro = load_part(out, "loro")
    frontend = load_part(out, "frontend")
    meta = {
        "github_sha": os.environ.get("GITHUB_SHA", ""),
        "github_ref": os.environ.get("GITHUB_REF", ""),
        "github_run_id": os.environ.get("GITHUB_RUN_ID", ""),
        "github_repository": os.environ.get("GITHUB_REPOSITORY", ""),
        "github_server_url": os.environ.get("GITHUB_SERVER_URL", "https://github.com"),
        "github_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT", ""),
    }
    run_url = ""
    if meta["github_repository"] and meta["github_run_id"]:
        run_url = f"{meta['github_server_url']}/{meta['github_repository']}/actions/runs/{meta['github_run_id']}"
    summary = {
        "meta": meta,
        "run_url": run_url,
        "api": api,
        "fanout": fanout,
        "explain": explain,
        "loro": loro,
        "frontend": frontend,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    (out / "summary.md").write_text(render_markdown(summary), encoding="utf-8")
    eprint(f"wrote {out / 'summary.md'}")
    required = [api, fanout, explain, loro, frontend]
    if any(part is None or not part.get("ok") for part in required):
        return 1
    return 0


def render_markdown(summary: dict) -> str:
    meta = summary["meta"]
    api = summary.get("api") or {}
    fanout = summary.get("fanout") or {}
    explain = summary.get("explain") or {}
    loro = summary.get("loro") or {}
    frontend = summary.get("frontend") or {}
    lines = [
        "# Flow 性能基线",
        "",
        "数字来自 GitHub Actions 上的 release API、PostgreSQL 16 和已登录的 Flow 页面。本文件由基准脚本生成，不是估计。",
        "",
        f"- 提交：`{meta.get('github_sha') or '未知'}`",
        f"- 分支：`{meta.get('github_ref') or '未知'}`",
        f"- 运行：{summary.get('run_url') or '本地状态目录，没有 Actions 运行号'}",
        f"- PostgreSQL：{explain.get('postgres') or '未采集'}",
        "",
        "## 接口",
        "",
        "时间是客户端从发出请求到读完响应，单位毫秒。`semantic_patch` 每批 50 个块（`create_node` + `insert_text`，100 个操作，等于单次上限）。1k 是 20 批，5k 是 100 批。",
        "",
        "| 项目 | 结果 |",
        "| --- | --- |",
        row(["注册", f"{fmt_ms(api.get('register_ms'))} ms"]),
        row(["登录", fmt_stats(api.get("login"))]),
        row(["创建工作区", f"{fmt_ms(api.get('create_workspace_ms'))} ms"]),
        row(["打开 Flow 开关", f"{fmt_ms(api.get('enable_flow_ms'))} ms"]),
        row(["创建空页面", fmt_stats(api.get("create_page"))]),
        row(["semantic_patch 1k 每批", fmt_stats((api.get("semantic_patch_1k") or {}).get("batch"))]),
        row(
            [
                "semantic_patch 1k 总墙钟",
                f"{fmt_ms((api.get('semantic_patch_1k') or {}).get('wall_ms'))} ms，完成 "
                f"{(api.get('semantic_patch_1k') or {}).get('batches_completed')}/"
                f"{(api.get('semantic_patch_1k') or {}).get('batches_expected')} 批",
            ]
        ),
        row(["semantic_patch 5k 每批", fmt_stats((api.get("semantic_patch_5k") or {}).get("batch"))]),
        row(
            [
                "semantic_patch 5k 总墙钟",
                f"{fmt_ms((api.get('semantic_patch_5k') or {}).get('wall_ms'))} ms，完成 "
                f"{(api.get('semantic_patch_5k') or {}).get('batches_completed')}/"
                f"{(api.get('semantic_patch_5k') or {}).get('batches_expected')} 批",
            ]
        ),
        row(
            [
                "5k 文档上再打一批",
                fmt_ms((api.get("semantic_patch_steady_on_5k") or {}).get("ms")) + " ms"
                if isinstance(api.get("semantic_patch_steady_on_5k"), dict)
                else (api.get("semantic_patch_steady_error") or "—"),
            ]
        ),
        row(["bootstrap 1k", fmt_stats((api.get("bootstrap_1k") or {}).get("latency"))]),
        row(["bootstrap 5k", fmt_stats((api.get("bootstrap_5k") or {}).get("latency"))]),
        row(
            [
                "bootstrap 5k 响应字节",
                fmt_stats((api.get("bootstrap_5k") or {}).get("response_bytes")).replace("_ms", ""),
            ]
        ),
        row(["搜索索引追上 5k 页", f"{fmt_ms((api.get('search_index_wait') or {}).get('wait_ms'))} ms"]),
        row(["搜索 flowbench5k", fmt_stats((api.get("search_unique") or {}).get("latency"))]),
        row(["搜索 benchblock", fmt_stats((api.get("search_common") or {}).get("latency"))]),
        row(["发表评论", fmt_stats((api.get("comment_create") or {}).get("latency"))]),
        row(
            [
                "评论列表",
                fmt_stats((api.get("comments_list") or {}).get("latency"))
                + f"，条数 {(api.get('comments_list') or {}).get('items')}",
            ]
        ),
        row(["移动小页面", f"{fmt_ms(api.get('move_small_ms'))} ms"]),
        row(["移动 1k 页面", f"{fmt_ms(api.get('move_1k_ms'))} ms"]),
        "",
    ]
    boot = (api.get("bootstrap_5k") or {}).get("last") or {}
    if boot:
        lines.extend(
            [
                f"5k bootstrap 最后一次：snapshot_seq={boot.get('snapshot_seq')}，head_seq={boot.get('head_seq')}，"
                f"tail={boot.get('tail_updates')}，snapshot base64 字符={boot.get('snapshot_base64_chars')}，"
                f"响应 {boot.get('response_bytes')} 字节。",
                "",
            ]
        )
    failures = []
    for label, block in (
        ("1k 写入", api.get("semantic_patch_1k")),
        ("5k 写入", api.get("semantic_patch_5k")),
    ):
        if isinstance(block, dict) and block.get("failures"):
            failures.append(f"- {label}：{block['failures'][0]}")
    for label, key in (
        ("搜索", "search_unique"),
        ("评论", "comment_create"),
        ("评论列表", "comments_list"),
        ("bootstrap 1k", "bootstrap_1k"),
        ("bootstrap 5k", "bootstrap_5k"),
    ):
        block = api.get(key) or {}
        if isinstance(block, dict) and block.get("error"):
            failures.append(f"- {label}：{block['error']}")
    if api.get("move_small_error"):
        failures.append(f"- 移动小页面：{api['move_small_error']}")
    if api.get("move_1k_error"):
        failures.append(f"- 移动 1k 页面：{api['move_1k_error']}")
    if failures:
        lines.extend(["接口里没有完成的部分：", "", *failures, ""])

    lines.extend(
        [
            "## 两个 API 进程的扇出",
            "",
            "进程 A 接受 REST `insert_block`。进程 B 只通过 LISTEN/NOTIFY 和 50ms 轮询把已提交更新送到自己的 WebSocket。"
            "端到端从发出 REST 前一刻算到 B 收到 `accepted`。响应后延迟是 HTTP 响应已经返回之后还要再等的时间，可能为负"
            "（帧比响应先到）。第一笔是预热，不计入。",
            "",
            f"- 状态：{'完成' if fanout.get('ok') else '失败'}",
            f"- 端到端：{fmt_stats(fanout.get('fanout_ms'))}",
            f"- HTTP：{fmt_stats(fanout.get('http_ms'))}",
            f"- 响应返回之后：{fmt_stats(fanout.get('after_http_response_ms'))}",
            "",
        ]
    )
    if fanout.get("error"):
        lines.extend([f"失败原因：{fanout['error']}", ""])

    lines.extend(["## Loro 加载、apply、快照", "", "同一台 runner 上的 release 二进制，进程内调用生产适配器，不经过 HTTP。", ""])
    if loro.get("error"):
        lines.extend([f"失败：{loro['error']}", ""])
    for size in loro.get("sizes") or []:
        lines.extend(
            [
                f"### {size.get('blocks')} 块",
                "",
                f"- apply {size.get('operations')} 个操作：{fmt_stats(size.get('apply'))}",
                f"- export snapshot：{fmt_stats(size.get('snapshot'))}，快照 {size.get('snapshot_bytes')} 字节",
                f"- load snapshot：{fmt_stats(size.get('load'))}",
                f"- 在已加载文档上 import 一条更新（{size.get('update_bytes')} 字节）：{fmt_stats(size.get('import_update'))}",
                "",
            ]
        )
    if not loro:
        lines.extend(["这一项没有产出。", ""])

    lines.extend(
        [
            "## EXPLAIN (ANALYZE)",
            "",
            "语句是这些路径上的 SQL，不是把 Rust 鉴权循环也算进去。执行时间单位毫秒，含缓冲区数字。",
            "",
            "| 查询 | 计划 | 执行 | 根节点 | 行数 | shared hit | shared read |",
            "| --- | ---: | ---: | --- | ---: | ---: | ---: |",
        ]
    )
    for name, item in (explain.get("queries") or {}).items():
        if not item.get("ok"):
            lines.append(row([name, "失败", item.get("error") or "", "", "", "", ""]))
            continue
        lines.append(
            row(
                [
                    name,
                    fmt_ms(item.get("planning_ms")),
                    fmt_ms(item.get("execution_ms")),
                    str(item.get("node_type") or ""),
                    str(item.get("actual_rows") if item.get("actual_rows") is not None else ""),
                    str(item.get("shared_hit_blocks") if item.get("shared_hit_blocks") is not None else ""),
                    str(item.get("shared_read_blocks") if item.get("shared_read_blocks") is not None else ""),
                ]
            )
        )
    lines.append("")
    if not explain:
        lines.extend(["没有 EXPLAIN 结果。", ""])

    lines.extend(["## 前端", "", "Playwright 打开已登录、Flow 开关已打开的对象页。Lighthouse 连的是同一套登录态；连不上时下面会写原因，Web Vitals 仍然单独记录。", ""])
    if frontend.get("error") and not frontend.get("pages"):
        lines.extend([f"失败：{frontend['error']}", ""])
    for page in frontend.get("pages") or []:
        vitals = page.get("vitals") or {}
        typing = page.get("typing") or {}
        lines.extend(
            [
                f"### {page.get('label')}",
                "",
                f"- 最终 URL：`{page.get('url') or ''}`",
                f"- 导航 TTFB {fmt_ms(vitals.get('ttfb_ms'))} ms，FCP {fmt_ms(vitals.get('fcp_ms'))} ms，"
                f"LCP {fmt_ms(vitals.get('lcp_ms'))} ms，DCL {fmt_ms(vitals.get('dom_content_loaded_ms'))} ms，"
                f"CLS {vitals.get('cls') if vitals.get('cls') is not None else '—'}",
                f"- 画布出现（脚本墙钟）{fmt_ms(page.get('canvas_wall_ms'))} ms，编辑器可输入 {fmt_ms(page.get('editor_wall_ms'))} ms",
                f"- 截图：{page.get('screenshot') or '无'}",
                f"- 打字：{fmt_stats(typing.get('latency')) if typing.get('ok') else (typing.get('error') or '未测')}",
                "",
            ]
        )
    lighthouse = frontend.get("lighthouse") or {}
    if lighthouse:
        lines.append(
            f"Lighthouse 性能分 {lighthouse.get('performance_score')}，"
            f"LCP {fmt_ms((lighthouse.get('metrics') or {}).get('largest-contentful-paint'))} ms，"
            f"CLS {(lighthouse.get('metrics') or {}).get('cumulative-layout-shift')}，"
            f"FCP {fmt_ms((lighthouse.get('metrics') or {}).get('first-contentful-paint'))} ms，"
            f"TBT {fmt_ms((lighthouse.get('metrics') or {}).get('total-blocking-time'))} ms。"
        )
        if lighthouse.get("error"):
            lines.append(f"Lighthouse 没有完成：{lighthouse['error']}")
        lines.append("")
    if not frontend:
        lines.extend(["没有前端结果。", ""])
    lines.extend(
        [
            "## 怎么读",
            "",
            "这是一次 Actions runner 上的基线，不是容量上限，也没有和 Notion 对比。扇出延迟包含最多约 50ms 的轮询兜底。侧栏仍然只拉 100 个对象，这次没有把那个上限算进延迟。",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("api", "fanout", "explain", "finalize"))
    parser.add_argument("--ids", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "api":
        if args.ids is None:
            parser.error("api requires --ids")
        try:
            run_api(args.ids, args.out)
        except Exception as err:
            write_part(args.out, "api", {"ok": False, "error": str(err)})
            raise
        return 0
    if args.command == "fanout":
        if args.ids is None:
            parser.error("fanout requires --ids")
        run_fanout(args.ids, args.out)
        return 0
    if args.command == "explain":
        if args.ids is None:
            parser.error("explain requires --ids")
        try:
            run_explain(args.ids, args.out)
        except Exception as err:
            write_part(args.out, "explain", {"ok": False, "error": str(err)})
            raise
        return 0
    return finalize(args.out)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ApiError as error:
        eprint(str(error))
        raise SystemExit(1) from error
