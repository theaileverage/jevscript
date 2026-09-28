#!/usr/bin/env python3
"""Print judgment expressions and call boundaries in compiled Jevscript IR.

Usage:
    request_groups.py FILE.jev [--all] [--jevscript PATH]
    jevscript compile FILE.jev | request_groups.py - [--all]

The compiler assigns each judgment expression a `request_group` (spec section 6.6).
An unsplit group sends one request with one state. An oversized inline `each`
can split into requests that each carry only their own item paths; a named
judgment carries its full parameters in every chunk. Other oversized groups
fail before sending. Calls are listed in evaluation order within each possible
path. This is a static inventory, not a prediction of which path runs. By
default only units written in FILE are shown; `--all` adds imported modules
and the `std` prelude. Non-relative `use` paths resolve through
`JEVSCRIPT_PATH`, as for `jevscript compile`.
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
    return f"each {text} (one question per runtime item)" if judge["each"] else text


class Units:
    def __init__(self, ir: dict[str, Any]) -> None:
        self.kind: dict[str, str] = {}
        for key, kind in (("judgments", "judgment"), ("defs", "def"), ("tasks", "task"), ("machines", "machine")):
            for unit in ir.get(key) or []:
                self.kind[unit["name"]] = kind
        self.capabilities = {need["name"]: need["kind"] for need in ir.get("needs") or []}
        self.modules = ir.get("modules") or []

    def capabilities_for(self, unit: str) -> dict[str, str]:
        module = max((m for m in self.modules if unit.startswith(m["alias"] + ".")),
                     key=lambda m: len(m["alias"]), default=None)
        return ({m["inner"]: m["kind"] for m in module.get("mapping") or []}
                if module else self.capabilities)

    def describe_call(self, call: dict[str, Any], capabilities: dict[str, str]) -> str | None:
        callee = call["callee"]
        if callee.get("node") == "name" and callee["name"] in self.kind:
            kind = self.kind[callee["name"]]
            suffix = " (one logical group)" if kind == "judgment" else ""
            return f"calls {kind} {callee['name']}{suffix}"
        if callee.get("node") == "field":
            target = callee["target"]
            if target.get("node") == "name":
                capability = target["name"]
                if capability in capabilities:
                    kind = capabilities[capability]
                    note = " (model call)" if kind == "llm" else ""
                    return f"effect {capability}.{callee['name']} [{kind}]{note}"
                return f"effect {capability}.{callee['name']} [handle]"
            return f"effect .{callee['name']} [handle]"
        return None


def events(node: Any, units: Units, capabilities: dict[str, str], target: str | None = None) -> list[tuple[int, str, Any]]:
    """Return possible judgment-expression and call rows in runtime evaluation order."""
    found: list[tuple[int, str, Any]] = []
    if isinstance(node, list):
        for child in node:
            found += events(child, units, capabilities)
    elif isinstance(node, dict):
        if "states" in node and "observe" in node:
            found += events(node.get("params"), units, capabilities)
            for state in node["states"]:
                found.append((line(state), f"state {state['name']} (possible step)", None))
                if state["done"]:
                    continue
                found += events(node["observe"], units, capabilities)
                for transition in state["transitions"]:
                    found.append((line(transition), f"guard {transition['event']} (if evaluated)", None))
                    found += events(transition.get("when"), units, capabilities)
                found += events(node.get("goal"), units, capabilities)
                for transition in state["transitions"]:
                    found.append((line(transition), f"description {transition['event']} (if enabled)", None))
                    found += events(transition["description"], units, capabilities)
                found.append((line(state), "machine Choice (if any event is enabled)", None))
                for transition in state["transitions"]:
                    if transition.get("body"):
                        found.append((line(transition), f"action {transition['event']} (if chosen)", None))
                        found += events(transition["body"], units, capabilities)
            return found
        if node.get("stmt") == "assign":
            target = node["root"] + "".join(f".{p}" if isinstance(p, str) else "[]" for p in node.get("path") or [])
        if "question" in node and "name" in node:
            return events(node["question"], units, capabilities, node["name"])
        kind = node.get("node")
        if kind == "call":
            for arg in node["args"]:
                found += events(arg["value"], units, capabilities)
            callee = node["callee"]
            found += events(callee["target"] if callee.get("node") == "field" else callee,
                            units, capabilities)
            description = units.describe_call(node, capabilities)
            if description:
                found.append((line(node), description, None))
            return found
        if kind == "field":
            found += events(node["target"], units, capabilities)
            target_node = node["target"]
            if target_node.get("node") == "name" and target_node["name"] in capabilities:
                capability = target_node["name"]
                found.append((line(node), f"effect {capability}.{node['name']} [{capabilities[capability]}]", None))
            elif target_node.get("node") == "name" and node["name"] in ("observe", "stop"):
                found.append((line(node), f"effect {target_node['name']}.{node['name']} [handle]", None))
            return found
        if kind == "focus":
            found += events(node["text"], units, capabilities)
            found += events(node["on"], units, capabilities)
            found.append((line(node), "calls focus (Jev)", None))
            return found
        if kind == "comprehension":
            found += events(node["iterable"], units, capabilities)
            found += events(node.get("test"), units, capabilities)
            found += events(node["expr"], units, capabilities)
            return found
        if "request_group" in node and "subject" in node:
            found += events(node["subject"], units, capabilities)
            found += events(node["verb"], units, capabilities)
            found += events(node.get("detail"), units, capabilities)
            start = node["span"]["start"]
            name = f"{target} = " if target else ""
            found.append((start["line"], f"{name}{node['subject']['state_path']} {verb_text(node)}", node["request_group"]))
            return found
        for key, child in node.items():
            if key != "span":
                found += events(child, units, capabilities, target if key == "value" else None)
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
            capabilities = units.capabilities_for(name)
            if kind == "judgment":
                header = f"\njudgment {name}({', '.join(unit['params'])})  line {line(unit)}: one logical group; state is every parameter in full"
            else:
                header = f"\n{kind} {name}  line {line(unit)}"
            if kind == "machine":
                header += ": each nonterminal step observes, checks guards, then may send a Choice over enabled events plus stay (spec 7.8)"
            out.append(header)
            rows = events(unit, units, capabilities)
            groups: dict[int, int] = {}
            for row_line, text, group in rows:
                if group is None:
                    out.append(f"  L{row_line:<4} {'':8} {text}")
                else:
                    groups[group] = groups.get(group, 0) + 1
                    out.append(f"  L{row_line:<4} group {group}  {text}")
            if groups:
                sizes = ", ".join(f"group {g}: {n} judgment expression{'s' if n != 1 else ''}" for g, n in sorted(groups.items()))
                out.append(f"  compiled logical groups: {len(groups)} ({sizes}); branches, loops and called units change which groups run and how often")
                out.append("  network request count depends on the profile question cap and runtime list sizes; an oversized group may fail before sending")
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
