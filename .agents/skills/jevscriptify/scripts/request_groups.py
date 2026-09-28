#!/usr/bin/env python3
"""Print every Jev question in a compiled Jevscript program with its request group.

Usage:
    request_groups.py FILE.jev [--all] [--jevscript PATH]
    jevscript compile FILE.jev | request_groups.py - [--all]

The compiler assigns each judgment expression a `request_group` (spec section 6.6).
Questions that share a group travel in one Jev request and see one state. Unit
calls, capability calls and `llm` generations are listed in source order too,
because they are what separate groups. By default only units written in FILE
are shown; `--all` adds imported modules and the `std` prelude. Non-relative
`use` paths resolve through `JEVSCRIPT_PATH`, as for `jevscript compile`.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from typing import Any


def compile_ir(source: str, jevscript: str) -> dict[str, Any]:
    if source == "-":
        return json.load(sys.stdin)
    result = subprocess.run([jevscript, "compile", source], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        sys.exit(result.returncode)
    return json.loads(result.stdout)


def line(node: dict[str, Any]) -> int:
    return node["span"]["start"]["line"]


def verb_text(judge: dict[str, Any]) -> str:
    verb = judge["verb"]
    kind = verb["verb"]
    if kind == "pick":
        text = f"pick ({len(verb['labels'])} labels)"
    elif kind == "rate":
        text = f"rate ({len(verb['levels'])} levels)"
    elif kind == "pick_among":
        text = "pick among" + (f" by {verb['by']}" if verb.get("by") else "") + (", none" if verb.get("allow_none") else "")
    else:
        text = kind
    return ("each " if judge["each"] else "") + text


class Units:
    def __init__(self, ir: dict[str, Any]) -> None:
        self.kind: dict[str, str] = {}
        for key, kind in (("judgments", "judgment"), ("defs", "def"), ("tasks", "task"), ("machines", "machine")):
            for unit in ir.get(key) or []:
                self.kind[unit["name"]] = kind
        self.capabilities = {need["name"]: need["kind"] for need in ir.get("needs") or []}

    def describe_call(self, call: dict[str, Any]) -> str | None:
        callee = call["callee"]
        if callee.get("node") == "name" and callee["name"] in self.kind:
            kind = self.kind[callee["name"]]
            suffix = " (one request)" if kind == "judgment" else ""
            return f"calls {kind} {callee['name']}{suffix}"
        if callee.get("node") == "field" and callee["target"].get("node") == "name":
            capability = callee["target"]["name"]
            if capability in self.capabilities:
                kind = self.capabilities[capability]
                note = " (model call)" if kind == "llm" else ""
                return f"effect {capability}.{callee['name']} [{kind}]{note}"
        return None


def events(node: Any, units: Units, target: str | None = None) -> list[tuple[int, int, str, Any]]:
    """(line, column, text, group) for each judge and call under `node`, in source order."""
    found: list[tuple[int, int, str, Any]] = []
    if isinstance(node, list):
        for child in node:
            found += events(child, units)
    elif isinstance(node, dict):
        if node.get("stmt") == "assign":
            target = node["root"] + "".join(f".{p}" if isinstance(p, str) else "[]" for p in node.get("path") or [])
        if node.get("node") == "judge":
            start = node["span"]["start"]
            name = f"{target} = " if target else ""
            found.append((start["line"], start["column"], f"{name}{node['subject']['state_path']} {verb_text(node)}", node["request_group"]))
        elif node.get("node") == "call":
            text = units.describe_call(node)
            if text:
                start = node["span"]["start"]
                found.append((start["line"], start["column"], text, None))
        for key, child in node.items():
            if key not in ("span", "subject", "verb"):
                found += events(child, units, target if key == "value" else None)
    return found


def report(ir: dict[str, Any], show_all: bool) -> str:
    units = Units(ir)
    out = [f"program {ir['program']}"]
    for key in ("judgments", "defs", "tasks", "machines"):
        for unit in ir.get(key) or []:
            name = unit["name"]
            if not show_all and "." in name:
                continue
            kind = units.kind[name]
            if kind == "judgment":
                out.append(f"\njudgment {name}({', '.join(unit['params'])})  line {line(unit)}: one request; state is every parameter in full")
                for result in unit["results"]:
                    question = result["question"]
                    out.append(f"  L{line(question):<4} group {question['request_group']}  {result['name']} = {question['subject']['state_path']} {verb_text(question)}")
                continue
            header = f"\n{kind} {name}  line {line(unit)}"
            if kind == "machine":
                header += ": each step sends one Choice over the enabled events, plus stay (spec 7.8)"
            out.append(header)
            rows = sorted(events(unit, units))
            groups: dict[int, int] = {}
            for row_line, _, text, group in rows:
                if group is None:
                    out.append(f"  L{row_line:<4} {'':8} {text}")
                else:
                    groups[group] = groups.get(group, 0) + 1
                    out.append(f"  L{row_line:<4} group {group}  {text}")
            if groups:
                sizes = ", ".join(f"group {g}: {n} question{'s' if n != 1 else ''}" for g, n in sorted(groups.items()))
                out.append(f"  own requests if every question runs: {len(groups)} ({sizes}); branches, loops and called units change the count")
    return "\n".join(out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("source", help="a .jev file, or - to read IR JSON from stdin")
    parser.add_argument("--all", action="store_true", help="include imported modules and the std prelude")
    parser.add_argument("--jevscript", default=os.environ.get("JEVSCRIPT_BIN") or shutil.which("jevscript") or "jevscript")
    args = parser.parse_args()
    print(report(compile_ir(args.source, args.jevscript), args.all))


if __name__ == "__main__":
    main()
