#!/usr/bin/env python3
"""Window and write-step attribution for Flow pages (the "Linux attribution run").

Subcommands:
  drive  seed pages of one shape through the REST API in 50-block batches and record, per
         write, its wall time and time window (needs a running API; see window-attribution.sh).
  table  join the three recordings into one per-step table:
           --wa       output of crates/collab-core/tests/window_attribution.rs (WA_* lines)
           --api-log  the API's JSON log with debug events of api::flow::collab::write
           --drive    JSON files written by `drive` (one per shape)

`table` is pure text processing and runs anywhere: python3 scripts/flow-bench/window_attribution.py selftest
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

BLOCKS_PER_BATCH = 50
DEFAULT_SIZES = (1000, 2700, 5000, 10000)

# Steps that do not overlap each other, in write order. Their sum is compared with the wall time
# of the REST call; everything else (REST pre-check, per-operation snapshots, response building,
# the title read, fanout) lands in "other".
TOP_STEPS = (
    "find_prior_update",
    "coordinator_acquire",
    "read_observed_head",
    "base_load",
    "export_base_snapshot",
    "seed_cache_fork",
    "isolated_apply",
    "candidate_load",
    "host_semantic_snapshot",
    "registry_prepare",
    "typed_references",
    "projection_json_and_text",
    "begin",
    "locked_staging",
    "commit",
)
# Steps nested inside `locked_staging`, shown indented and not summed.
NESTED_STEPS = (
    "locked_write_cte",
    "reconcile_body_assets",
    "reconcile_child_pages",
    "reconcile_block_registry",
)


def eprint(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------------------------
# table: parsing
# ---------------------------------------------------------------------------------------------


def parse_wa(text: str) -> dict:
    """Parses WA_LIB / WA_WORKER lines into {(kind, shape, blocks): [fields...]} (strings)."""
    cells: dict = {}
    build = "unknown"
    for line in text.splitlines():
        parts = line.rstrip("\n").split("\t")
        if not parts:
            continue
        if parts[0] == "WA_HEADER" and len(parts) > 1:
            build = parts[1].removeprefix("build=")
        elif parts[0] in ("WA_LIB", "WA_WORKER") and len(parts) >= 3:
            try:
                blocks = int(parts[2])
            except ValueError:
                continue
            cells[(parts[0], parts[1], blocks)] = parts[3:]
    return {"build": build, "cells": cells}


def parse_timestamp(value: str) -> float | None:
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.timestamp()


def parse_api_log(text: str) -> list[dict]:
    """Returns write-step events: {"t", "kind": "step"|"window", ...} from the JSON log."""
    events = []
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        fields = record.get("fields")
        if not isinstance(fields, dict):
            continue
        stamp = parse_timestamp(str(record.get("timestamp", "")))
        if stamp is None:
            continue
        message = fields.get("message")
        if message == "flow write step" and isinstance(fields.get("elapsed_us"), int):
            events.append({"t": stamp, "kind": "step", "step": fields.get("step"), "us": fields["elapsed_us"]})
        elif message == "flow write isolated window":
            window = fields.get("window_cpu_us")
            events.append({"t": stamp, "kind": "window", "us": window if isinstance(window, int) else None})
    return events


def median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def fmt_ms_from_us(value) -> str:
    if value is None:
        return "-"
    return f"{float(value) / 1000.0:.2f}"


def fmt_ms(value) -> str:
    if value is None:
        return "-"
    return f"{float(value):.1f}"


def render_table(headers: list[str], rows: list[list[str]]) -> str:
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))
    def line(cells: list[str]) -> str:
        return "  ".join(cell.ljust(widths[i]) if i == 0 else cell.rjust(widths[i]) for i, cell in enumerate(cells))
    out = [line(headers), line(["-" * w for w in widths])]
    out.extend(line(row) for row in rows)
    return "\n".join(out)


def window_section(wa: dict) -> str:
    cells = wa["cells"]
    shapes = sorted({shape for (_k, shape, _b) in cells}, key=lambda s: (s != "flat", s))
    out = [f"Window CPU in ms, collab-core build: {wa['build']} (median of the runs; ceiling 50)"]
    for shape in shapes:
        sizes = sorted({b for (_k, s, b) in cells if s == shape})
        rows = []
        for blocks in sizes:
            lib = cells.get(("WA_LIB", shape, blocks))
            worker = cells.get(("WA_WORKER", shape, blocks))
            row = [str(blocks)]
            if lib and len(lib) >= 7:
                row += [fmt_ms_from_us(lib[i]) for i in (1, 2, 3, 4)]
            else:
                row += ["-"] * 4
            if worker and len(worker) >= 7:
                ok_runs = worker[0]
                row += [
                    fmt_ms_from_us(worker[2]) if worker[0] != "0" else "-",
                    f"{fmt_ms_from_us(worker[3])}..{fmt_ms_from_us(worker[4])}" if worker[0] != "0" else "-",
                    fmt_ms_from_us(worker[5]) if worker[0] != "0" else "-",
                    f"{ok_runs}/{worker[1]} {worker[6]}",
                ]
            else:
                row += ["-"] * 3 + ["not run"]
            rows.append(row)
        out.append(f"\n[{shape}] 50-block batch on a page of N blocks")
        out.append(
            render_table(
                ["blocks", "import", "snapshot", "check", "window(lib)", "window_cpu_us", "min..max", "child total", "worker runs/status"],
                rows,
            )
        )
    return "\n".join(out)


def drive_samples(drive: dict, size: int) -> list[dict]:
    """The writes that take the page to `size` blocks and the two after it (as in the library cell)."""
    wanted = {size, size + BLOCKS_PER_BATCH, size + 2 * BLOCKS_PER_BATCH}
    return [w for w in drive.get("writes", []) if w.get("ok") and w.get("blocks_after") in wanted]


def steps_for_write(events: list[dict], write: dict) -> dict:
    steps: dict = {}
    window = None
    for event in events:
        if write["t_start"] - 0.05 <= event["t"] <= write["t_end"] + 0.05:
            if event["kind"] == "step":
                steps[event["step"]] = steps.get(event["step"], 0) + event["us"]
            else:
                window = event["us"]
    return {"steps": steps, "window_cpu_us": window}


def api_section(drive: dict, events: list[dict], sizes: tuple[int, ...]) -> str:
    shape = drive.get("shape", "?")
    per_size: dict = {}
    for size in sizes:
        collected = [(w, steps_for_write(events, w)) for w in drive_samples(drive, size)]
        per_size[size] = collected
    header = ["step (median ms)"] + [str(s) for s in sizes]
    rows: list[list[str]] = []

    def value_row(label: str, getter, ms_from_us: bool) -> list[str]:
        row = [label]
        for size in sizes:
            values = [v for v in (getter(w, s) for w, s in per_size[size]) if v is not None]
            m = median(values)
            row.append(fmt_ms_from_us(m) if ms_from_us else fmt_ms(m))
        return row

    rows.append(value_row("REST call, wall", lambda w, s: w["elapsed_ms"], False))
    for step in TOP_STEPS:
        rows.append(value_row(step, lambda w, s, step=step: s["steps"].get(step), True))
        if step == "locked_staging":
            for nested in NESTED_STEPS:
                rows.append(value_row("  " + nested, lambda w, s, nested=nested: s["steps"].get(nested), True))

    def other(w, s):
        total_us = w["elapsed_ms"] * 1000.0
        known = sum(s["steps"].get(step, 0) for step in TOP_STEPS)
        return total_us - known if s["steps"] else None

    rows.append(value_row("other (REST pre-check, response, ...)", other, True))
    rows.append(value_row("window_cpu_us (worker, from API log)", lambda w, s: s["window_cpu_us"], True))
    rows.append(["samples"] + [str(len(per_size[size])) for size in sizes])
    notes = []
    for size in sizes:
        if not per_size[size]:
            notes.append(f"  {size}: no successful write there; {drive.get('stopped') or 'seeding did not get that far'}")
    text = f"[{shape}] API write steps (wall clock, ms)\n" + render_table(header, rows)
    if notes:
        text += "\n" + "\n".join(notes)
    return text


def run_table(wa_path: Path | None, api_log: Path | None, drive_paths: list[Path], sizes: tuple[int, ...]) -> str:
    sections = []
    if wa_path is not None and wa_path.exists():
        sections.append(window_section(parse_wa(wa_path.read_text(encoding="utf-8", errors="replace"))))
    events = []
    if api_log is not None and api_log.exists():
        events = parse_api_log(api_log.read_text(encoding="utf-8", errors="replace"))
    for path in drive_paths:
        if path.exists():
            sections.append(api_section(json.loads(path.read_text(encoding="utf-8")), events, sizes))
    return "\n\n".join(sections) + "\n"


# ---------------------------------------------------------------------------------------------
# drive: HTTP seeding
# ---------------------------------------------------------------------------------------------


def psql_scalar(database_url: str, sql: str) -> str:
    result = subprocess.run(
        ["psql", database_url, "-X", "-q", "-t", "-A", "-v", "ON_ERROR_STOP=1", "-c", sql],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def batch_operations(start: int) -> list[dict]:
    operations = []
    for index in range(start, start + BLOCKS_PER_BATCH):
        block_id = f"b{index:05d}"
        operations.append({"op": "create_node", "id": block_id, "parent": None, "index": 0, "kind": "block"})
        operations.append({"op": "insert_text", "id": block_id, "index": 0, "text": f"benchblock {index} alpha"})
    return operations


def run_drive(args: argparse.Namespace) -> int:
    import bench  # same directory; reuses the benchmark's HTTP helpers

    origin = os.environ["FLOW_BENCH_API_ORIGIN"]
    database_url = os.environ["FLOW_BENCH_DATABASE_URL"]
    email = f"window-attr-{os.urandom(4).hex()}@bench.sylvode.test"
    password = f"Aa1!{os.urandom(16).hex()}Z"
    registered, _ms, _size = bench.call(
        "POST", f"{origin}/api/v1/auth/register", None, {"email": email, "password": password, "name": "Window Attribution"}, 60
    )
    token = registered["tokens"]["access_token"]
    workspace, _ms, _size = bench.call(
        "POST", f"{origin}/api/v1/workspaces", token, {"slug": f"wa-{os.urandom(4).hex()}", "name": "Window Attribution"}, 60
    )
    workspace_id = workspace["id"]
    bench.call(
        "PUT",
        f"{origin}/api/v1/workspaces/{workspace_id}/features/flow",
        token,
        {"enabled": True, "default_member_level": "edit", "idempotency_key": bench.new_key()},
        60,
    )

    if args.shape == "flat":
        page, _ms = bench.create_page(origin, token, workspace_id, "Window attribution flat")
        object_id = page["id"]
    else:
        scope_id = psql_scalar(
            database_url, f"SELECT scope_id FROM flow_workspace_scope_bindings WHERE workspace_id = '{workspace_id}'"
        )
        data, _ms, _size = bench.call(
            "POST",
            f"{origin}/api/v2/flow/objects",
            token,
            {
                "scope_id": scope_id,
                "object_type": "page",
                "title": "Window attribution structured",
                "idempotency_key": bench.new_key(),
                "editor_model": 4,
            },
            60,
        )
        object_id = data["object"]["id"]

    sizes = sorted(args.sizes)
    top = max(sizes) + 2 * BLOCKS_PER_BATCH
    writes: list[dict] = []
    stopped = None
    started = time.monotonic()
    blocks = 0
    url = f"{origin}/api/v1/flow/objects/{object_id}/commands"
    while blocks < top:
        if time.monotonic() - started > args.max_seconds:
            stopped = f"time budget of {args.max_seconds}s reached at {blocks} blocks"
            break
        body = {
            "command": {"type": "semantic_patch", "payload": {"operations": batch_operations(blocks)}},
            "idempotency_key": bench.new_key(),
        }
        t_start = time.time()
        try:
            _data, elapsed, _retries = bench.send_with_contention_retry(lambda: bench.call("POST", url, token, body, 300))
        except bench.ApiError as err:
            stopped = f"write at {blocks} blocks failed: {str(err)[:300]}"
            writes.append({"blocks_before": blocks, "blocks_after": blocks + BLOCKS_PER_BATCH, "ok": False})
            break
        t_end = time.time()
        blocks += BLOCKS_PER_BATCH
        writes.append(
            {"blocks_before": blocks - BLOCKS_PER_BATCH, "blocks_after": blocks, "ok": True, "t_start": t_start, "t_end": t_end, "elapsed_ms": elapsed}
        )
        if blocks % 500 == 0:
            eprint(f"  {args.shape}: {blocks} blocks, last write {elapsed:.0f} ms")
    result = {"shape": args.shape, "object_id": object_id, "writes": writes, "stopped": stopped}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8")
    eprint(f"wrote {args.out}" + (f" (stopped: {stopped})" if stopped else ""))
    return 0


# ---------------------------------------------------------------------------------------------
# selftest: exercises the table on a recorded sample
# ---------------------------------------------------------------------------------------------

SAMPLE_WA = """WA_HEADER\tbuild=release
WA_LIB\tflat\t1000\t7\t900\t4100\t60\t5100\t4700\t5400
WA_WORKER\tflat\t1000\t7\t7\t18000\t17500\t19000\t26000\tok
WA_LIB\tflat\t10000\t7\t19000\t33000\t900\t54700\t53000\t58000
WA_WORKER\tflat\t10000\t0\t7\t0\t0\t0\t0\tCpuCeiling
WA_LIB\tstructured\t1000\t7\t1500\t6000\t100\t7600\t7100\t8000
WA_WORKER\tstructured\t1000\t7\t7\t25000\t24000\t26000\t33000\tok
"""


def selftest() -> int:
    wa = parse_wa(SAMPLE_WA)
    assert wa["build"] == "release"
    assert wa["cells"][("WA_LIB", "flat", 1000)][4] == "5100"
    text = window_section(wa)
    assert "5.10" in text and "CpuCeiling" in text and "[structured]" in text, text

    log = "\n".join(
        json.dumps(
            {
                "timestamp": ts,
                "level": "DEBUG",
                "fields": fields,
                "target": "api::flow::collab::write",
            }
        )
        for ts, fields in [
            ("2026-11-21T10:00:00.100000Z", {"message": "flow write step", "step": "base_load", "elapsed_us": 2000}),
            ("2026-11-21T10:00:00.300000Z", {"message": "flow write step", "step": "isolated_apply", "elapsed_us": 90000}),
            ("2026-11-21T10:00:00.300100Z", {"message": "flow write isolated window", "window_cpu_us": 18000}),
            ("2026-11-21T10:00:00.500000Z", {"message": "flow write step", "step": "locked_staging", "elapsed_us": 30000}),
            ("2026-11-21T10:00:00.510000Z", {"message": "flow write step", "step": "reconcile_block_registry", "elapsed_us": 12000}),
            ("2026-11-21T11:00:00.000000Z", {"message": "flow write step", "step": "base_load", "elapsed_us": 999999}),
        ]
    ) + "\nnot json\n"
    events = parse_api_log(log)
    assert len(events) == 6
    base = datetime(2026, 11, 21, 10, 0, 0, tzinfo=timezone.utc).timestamp()
    drive = {
        "shape": "flat",
        "stopped": "write at 1050 blocks failed: limit_exceeded",
        "writes": [
            {"blocks_before": 950, "blocks_after": 1000, "ok": True, "t_start": base, "t_end": base + 0.6, "elapsed_ms": 600.0},
        ],
    }
    table = api_section(drive, events, (1000, 2700))
    assert "window_cpu_us (worker, from API log)" in table
    assert "18.00" in table and "999.99" not in table, table
    assert "2700: no successful write there" in table, table
    other = [line for line in table.splitlines() if line.startswith("other")][0]
    # 600 ms wall minus (2 + 90 + 30) ms of top-level steps = 478 ms
    assert "478.00" in other, other
    assert run_table(None, None, [], DEFAULT_SIZES) == "\n"
    print(text)
    print()
    print(table)
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    d = sub.add_parser("drive")
    d.add_argument("--shape", choices=("flat", "structured"), required=True)
    d.add_argument("--sizes", type=lambda s: [int(x) for x in s.split(",")], default=list(DEFAULT_SIZES))
    d.add_argument("--max-seconds", type=int, default=1500)
    d.add_argument("--out", type=Path, required=True)
    t = sub.add_parser("table")
    t.add_argument("--wa", type=Path)
    t.add_argument("--api-log", type=Path)
    t.add_argument("--drive", type=Path, action="append", default=[])
    t.add_argument("--sizes", type=lambda s: tuple(int(x) for x in s.split(",")), default=DEFAULT_SIZES)
    sub.add_parser("selftest")
    args = parser.parse_args()
    if args.command == "drive":
        return run_drive(args)
    if args.command == "table":
        sys.stdout.write(run_table(args.wa, args.api_log, args.drive, args.sizes))
        return 0
    return selftest()


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    sys.exit(main())
