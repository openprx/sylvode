#!/usr/bin/env python3
"""Build the protocol 3 sample install requests: one `ui` package and one `wasm` package."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

MAX_AMOUNT_CENTS = 1_000_000_000_000
EXAMPLE = Path(__file__).resolve().parent


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(data):
    return base64.b64encode(data).decode("ascii")


def capability(name, permission):
    return {
        "name": name,
        "permission": permission,
        "input_schema": {"type": "object", "additionalProperties": False},
        "output_schema": {
            "type": "object",
            "properties": {"amount_cents": {"type": "integer", "minimum": 0, "maximum": MAX_AMOUNT_CENTS}},
            "required": ["amount_cents"],
            "additionalProperties": False,
        },
    }


def contribution(cid, kind, slot, label, view, route, form_key):
    return {"id": cid, "kind": kind, "slot": slot, "label": label,
            "view": view, "route": route, "form_key": form_key}


def base_manifest(key, name, entry, form_key):
    return {
        "schema_version": "openpr.plugin.v3",
        "key": key,
        "name": name,
        "version": "1.0.0",
        "publisher": "Sylvode examples",
        "host_protocol": {"min": 3, "max": 3},
        "profile": "ui",
        "bridge_version": 2,
        "entry": {"path": "panel.html", "size": len(entry), "sha256": digest(entry)},
        "wasm": None,
        "contributions": [
            contribution("board", "view", "project.view", "Order board", "board", "board", None),
            contribution("record_summary", "panel", "form.record.detail", "Order summary", "summary", None, form_key),
            contribution("list_tool", "action", "form.list.toolbar", "Order totals", "totals", None, form_key),
        ],
        "reads": [{"id": "orders", "form_key": form_key, "fields": ["quantity", "unit_price", "amount"]}],
        "capabilities": [],
    }


def build_wasm():
    with tempfile.TemporaryDirectory() as temp:
        module = Path(temp) / "plugin.wasm"
        subprocess.run(
            [
                "rustc", str(EXAMPLE / "guest.rs"),
                "--edition=2024", "--target=wasm32-wasip1",
                "--crate-type=cdylib", "-O", "-C", "panic=abort", "-C", "strip=symbols",
                "-C", "link-arg=--no-entry", "-o", str(module),
            ],
            check=True,
            cwd=EXAMPLE,
        )
        return module.read_bytes()


def write(output, name, package):
    (output / name).write_text(json.dumps(package, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def build(output, form_key):
    entry = (EXAMPLE / "panel-view.html").read_text(encoding="utf-8").encode("utf-8")
    output.mkdir(parents=True, exist_ok=True)

    ui = base_manifest("order_board", "Order board", entry, form_key)
    write(output, "extension-ui-1.0.0.json", {"manifest": ui, "web_base64": encoded(entry)})

    wasm = build_wasm()
    manifest = base_manifest("order_board_amount", "Order board with amount", entry, form_key)
    manifest["profile"] = "wasm"
    manifest["wasm"] = {
        "abi": "openpr.plugin.v1",
        "path": "plugin.wasm",
        "size": len(wasm),
        "sha256": digest(wasm),
        "form_key": form_key,
    }
    manifest["capabilities"] = [
        capability("amount.preview", "record.read"),
        capability("amount.apply", "record.update"),
    ]
    write(output, "extension-wasm-1.0.0.json",
          {"manifest": manifest, "wasm_base64": encoded(wasm), "web_base64": encoded(entry)})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--form-key", default="order_line")
    args = parser.parse_args()
    build(args.output, args.form_key)
