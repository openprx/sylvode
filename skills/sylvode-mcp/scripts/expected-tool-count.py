#!/usr/bin/env python3
"""Print the MCP registry baseline after proving it matches the live binary.

With `--names` print the live tool names instead, one per line; `--category NAME` restricts them to
one catalogue category. Both come from `list-tools --json`.
"""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys


repo = Path(__file__).resolve().parents[3]
baseline_path = repo / "apps/mcp-server/tool-registry-baseline.json"
baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
expected_count = baseline.get("count")
expected_hash = baseline.get("names_sha256")
if (
    baseline.get("schema_version") != "sylvode.mcp-tool-registry-baseline.v1"
    or baseline.get("source") != "mcp_server::get_all_tool_definitions"
    or not isinstance(expected_count, int)
    or expected_count <= 0
    or not isinstance(expected_hash, str)
    or re.fullmatch(r"[0-9a-f]{64}", expected_hash) is None
):
    raise SystemExit(f"invalid MCP tool registry baseline: {baseline_path}")

environment = os.environ.copy()
environment.setdefault("CARGO_BUILD_JOBS", "4")
build = subprocess.run(
    ["cargo", "build", "-q", "-p", "mcp-server", "--bin", "list-tools"],
    cwd=repo,
    env=environment,
    text=True,
    capture_output=True,
    check=False,
)
if build.returncode != 0:
    raise SystemExit(f"cannot build live MCP registry:\n{build.stdout}{build.stderr}")

target_dir = Path(environment.get("CARGO_TARGET_DIR", repo / "target"))
if not target_dir.is_absolute():
    target_dir = repo / target_dir
binary = target_dir / "debug/list-tools"
live = subprocess.run([binary, "--json"], cwd=repo, text=True, capture_output=True, check=False)
if live.returncode != 0:
    raise SystemExit(f"cannot execute live MCP registry:\n{live.stdout}{live.stderr}")

rows = json.loads(live.stdout)
names = sorted({row["name"] for row in rows})
live_hash = hashlib.sha256("\n".join(names).encode()).hexdigest()
if len(names) != len(rows) or len(names) != expected_count or live_hash != expected_hash:
    raise SystemExit(
        "MCP tool registry drift: "
        f"baseline count/hash={expected_count}/{expected_hash}, "
        f"live count/hash={len(names)}/{live_hash}"
    )

if "--names" in sys.argv[1:]:
    category = sys.argv[sys.argv.index("--category") + 1] if "--category" in sys.argv[1:] else None
    for row in rows:
        if category is None or row["category"] == category:
            print(row["name"])
else:
    print(expected_count)
