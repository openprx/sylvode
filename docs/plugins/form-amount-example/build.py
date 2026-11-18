#!/usr/bin/env python3
"""Build import-free WASM guests and matching install requests for the Form sample."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def asset(path, data):
    return {"path": path, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def capability(name, permission):
    return {
        "name": name,
        "permission": permission,
        "input_schema": {"type": "object", "additionalProperties": False},
        "output_schema": {
            "type": "object",
            "properties": {"amount_cents": {"type": "integer", "minimum": 0, "maximum": 1_000_000_000_000}},
            "required": ["amount_cents"],
            "additionalProperties": False,
        },
    }


def encoded(data):
    return base64.b64encode(data).decode("ascii")


def build(output, form_key, include_layout):
    example = Path(__file__).resolve().parent
    html = (example / "panel.html").read_text(encoding="utf-8")
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        module = Path(temp) / "plugin.wasm"
        for major in ((3,) if include_layout else (1, 2)):
            flags = ["--cfg", "extra_fee"] if major == 2 else []
            subprocess.run(
                [
                    "rustc", "guest.rs" if include_layout else str(example / "guest.rs"),
                    "--edition=2024", "--target=wasm32-wasip1",
                    "--crate-type=cdylib", "-O", "-C", "panic=abort", "-C", "strip=symbols",
                    "-C", "link-arg=--no-entry", "-o", str(module), *flags,
                ],
                check=True,
                cwd=example,
            )
            wasm = module.read_bytes()
            if major == 3:
                web = (example / "panel-layout.html").read_text(encoding="utf-8")
            elif major == 2:
                web = html.replace("quantity and unit price.", "quantity and unit price, plus a 1.00 handling fee.")
            else:
                web = html
            web = web.encode("utf-8")
            version = f"{major}.0.0"
            manifest = {
                "schema_version": "openpr.plugin.v2",
                "key": "order_amount",
                "name": {1: "Order amount", 2: "Order amount with handling fee", 3: "Order amount controls"}[major],
                "version": version,
                "publisher": "Sylvode examples",
                "host_protocol": {"min": 1, "max": 1},
                "wasm_abi": "openpr.plugin.v1",
                "wasm": asset("plugin.wasm", wasm),
                "ui": {
                    "entry": asset("panel.html", web),
                    "slot": "form.record.detail",
                    "bridge_version": 1,
                    "form_key": form_key,
                },
                "capabilities": [
                    capability("amount.preview", "record.read"),
                    capability("amount.apply", "record.update"),
                ],
            }
            if major == 3:
                manifest["host_protocol"] = {"min": 2, "max": 2}
                manifest["ui"]["contributions"] = [
                    {"id": "detail_panel", "slot": "form.record.detail", "label": "Order amount panel"},
                    {"id": "amount_action", "slot": "form.record.actions", "label": "Calculate order amount"},
                ]
            package = {"manifest": manifest, "wasm_base64": encoded(wasm), "web_base64": encoded(web)}
            (output / f"order-amount-{version}.json").write_text(
                json.dumps(package, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
            )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--form-key", default="order_line")
    parser.add_argument("--layout", action="store_true", help="emit the host-protocol-2 layout sample only")
    args = parser.parse_args()
    build(args.output, args.form_key, args.layout)
