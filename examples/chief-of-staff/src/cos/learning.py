"""Learning: turn repeated work into playbooks that go live on their own.

A pass runs ``learning.learn`` (Jevscript) over the ledger. For each draft it
returns, the host:

1. writes the source as a new version under ``data/playbooks/``;
2. compiles it with ``jevscript check`` (a draft that does not compile falls
   back to the template rendering of the same evidence);
3. verifies it: replays the recorded intake of every past task it claims
   (zero model calls; the replay must reproduce the recorded route), runs the
   playbook on those requests and on unrelated ones, and hands the rows to
   ``playbooks.verdict`` (Jevscript, code only), which decides;
4. registers it live when the verdict passes. There is no approval step: the
   person reviews afterwards (``cos playbook list/show``) and can disable or
   revert any playbook in one command. Every step lands in the audit trail.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .state.home import append_jsonl, iso, now, read_json, read_jsonl, write_json
from .jevbin import find_jevscript

if TYPE_CHECKING:  # pragma: no cover
    from .host import Host

#: The same category descriptions `learning.categorize` judges with; a
#: playbook's trigger condition is its category's description.
CATEGORY_TRIGGERS = {
    "dependency_update": "asks to update, bump or pin a dependency, toolchain or tool version",
    "ci_repair": "asks to fix a failing build, test run, lint check or CI job",
    "docs_change": "asks to write or correct documentation, comments or a README",
    "release_task": "asks to cut, tag or prepare a release or a changelog",
    "cleanup": "asks to remove dead code, warnings or formatting problems",
    "investigation": "asks to investigate, reproduce or explain something without changing code",
    "feature": "asks to build new behavior",
}


def _jev_text(value: str) -> str:
    """Escape a value for a Jevscript text literal (braces interpolate)."""
    return value.replace("\\", "\\\\").replace('"', "'").replace("{", "{{").replace("}", "}}").replace("\n", " ")


def _jev_block(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"""', "'''").replace("{", "{{").replace("}", "}}")


def render_playbook(spec: dict[str, Any], version: int = 1) -> str:
    name = spec["name"]
    category = spec["category"]
    trigger = CATEGORY_TRIGGERS.get(category, spec.get("trigger") or category.replace("_", " "))
    projects = {p for p in spec.get("projects") or [] if p}
    project = f'"{_jev_text(spec["project"])}"' if len(projects) == 1 and spec.get("project") else "request.project"
    obstacles = [o for o in spec.get("obstacles") or [] if o]
    notes = f"This is a routine {category.replace('_', ' ')} task; this procedure has succeeded {len(spec.get('evidence') or [])} times."
    if obstacles:
        notes += "\nPast runs lost time to these obstacles; avoid them up front:\n" + "\n".join(f"- {o}" for o in dict.fromkeys(obstacles))
    evidence = ", ".join(spec.get("evidence") or [])
    return f'''program {name}

# Learned playbook `{name}`, version {version}, drafted {iso()[:10]} from past
# tasks: {evidence}. It went live after replay verification; review it with
# `cos playbook show {name}` and turn it off with `cos playbook disable {name}`.

in request: {{ text, project }}
out plan

task main budget calls 2:
  text = request.text
  applies = text feels "{_jev_text(trigger)}":
    focus "Whether the request is an instance of this routine procedure, not merely a mention of it."
  plan = {{ matches: false, playbook: "{name}" }}
  if applies > 0.6:
    plan = {{
      matches: true,
      playbook: "{name}",
      project: {project},
      kind: "{_jev_text(spec.get("kind") or "ship")}",
      effort: "{_jev_text(spec.get("effort") or "low")}",
      profile: "{_jev_text(spec.get("profile") or "default")}",
      notes: """{_jev_block(notes)}"""
    }}
'''


def compile_check(path: Path) -> tuple[bool, str]:
    result = subprocess.run([find_jevscript(), "check", str(path)], capture_output=True, text=True, check=False, timeout=60)
    errors = [line for line in result.stderr.splitlines() if ": warning:" not in line and line.strip()]
    return result.returncode == 0, "\n".join(errors)


def replay(recording: str) -> dict[str, Any] | None:
    """Replay a recording with no model calls; the final pause, or None."""
    if not recording or not Path(recording).exists():
        return None
    result = subprocess.run([find_jevscript(), "replay", recording], capture_output=True, text=True, check=False, timeout=120)
    if result.returncode != 0:
        return None
    lines = [line for line in result.stdout.splitlines() if line.strip().startswith("{")]
    return json.loads(lines[-1]) if lines else None


class Playbooks:
    """The registry of learned playbooks and their audit trail."""

    def __init__(self, host: "Host") -> None:
        self.host = host
        self.dir = host.home.data / "playbooks"
        self.path = self.dir / "registry.json"
        self.audit_path = self.dir / "audit.jsonl"

    def registry(self) -> dict[str, dict[str, Any]]:
        return read_json(self.path, {})

    def active(self) -> list[dict[str, Any]]:
        return [
            {"name": name, "category": row["category"], "trigger": row["trigger"]}
            for name, row in self.registry().items()
            if row.get("active")
        ]

    def audit(self, event: str, name: str, **fields: Any) -> None:
        append_jsonl(self.audit_path, {"at": iso(), "event": event, "playbook": name, **fields})

    def audit_trail(self, name: str | None = None) -> list[dict[str, Any]]:
        return [r for r in read_jsonl(self.audit_path) if name is None or r["playbook"] == name]

    def register(self, name: str, category: str, path: Path, version: int, spec: dict[str, Any], verdict: dict[str, Any]) -> None:
        rows = self.registry()
        row = rows.setdefault(name, {"category": category, "versions": []})
        row["trigger"] = CATEGORY_TRIGGERS.get(category, category)
        row["versions"].append({"version": version, "path": str(path), "at": iso(), "verdict": verdict, "evidence": spec.get("evidence", [])})
        row.update(active=True, version=version, path=str(path))
        write_json(self.path, rows)
        self.audit("live", name, version=version, path=str(path), verdict=verdict)

    def set_active(self, name: str, active: bool) -> None:
        rows = self.registry()
        if name not in rows:
            raise KeyError(f"no playbook `{name}`")
        rows[name]["active"] = active
        write_json(self.path, rows)
        self.audit("enabled" if active else "disabled", name)

    def revert(self, name: str) -> dict[str, Any]:
        rows = self.registry()
        row = rows.get(name)
        if row is None:
            raise KeyError(f"no playbook `{name}`")
        earlier = [v for v in row["versions"] if v["version"] < row["version"]]
        if not earlier:
            row["active"] = False
            self.audit("reverted", name, to=None)
        else:
            previous = earlier[-1]
            row.update(version=previous["version"], path=previous["path"], active=True)
            self.audit("reverted", name, to=previous["version"])
        write_json(self.path, rows)
        return row

    def next_version(self, name: str) -> int:
        row = self.registry().get(name)
        existing = [int(m.group(1)) for p in self.dir.glob(f"{name}.v*.jev") if (m := re.search(r"\.v(\d+)\.jev$", p.name))]
        return max([0, *existing, *(v["version"] for v in (row or {}).get("versions", []))]) + 1

    def path_of(self, name: str) -> Path | None:
        row = self.registry().get(name)
        return Path(row["path"]) if row and row.get("active") else None


class Learning:
    def __init__(self, host: "Host") -> None:
        self.host = host
        self.playbooks = Playbooks(host)
        self.state_path = host.home.state / "learning.json"

    def due(self) -> bool:
        last = read_json(self.state_path, {"ts": 0})["ts"]
        return len(self.host.ledger.tasks_since(last)) >= self.host.home.config["learn_every"]

    def run_pass(self) -> dict[str, Any]:
        write_json(self.state_path, {"ts": now(), "at": iso()})
        entries = self.host.ledger.learning_entries()
        policy = self.host.policy()
        if len(entries) < policy["min_repeats"]:
            return {"patterns": [], "drafts": [], "adopted": []}
        episode = self.host.run_task(
            "learn",
            {"phase": "find", "entries": entries, "playbooks": self.playbooks.active() + self._covered(), "policy": policy},
            subject="learning",
        )
        result = episode.result or {"patterns": [], "drafts": []}
        for pattern in result.get("patterns", []):
            self.playbooks.audit("pattern", f"pb_{pattern['category']}", **pattern)
        adopted = [self.adopt(draft) for draft in result.get("drafts", [])]
        if any(a["live"] for a in adopted):
            live = ", ".join(a["name"] for a in adopted if a["live"])
            self.host.decisions.notify(f"Learned and switched on: {live}. Review with `cos playbook list`; any one can be turned off.")
        return {**result, "adopted": adopted}

    def _covered(self) -> list[dict[str, Any]]:
        """Disabled playbooks still count as covered: the person turned them
        off, so learning must not quietly draft them again."""
        return [
            {"name": n, "category": r["category"], "trigger": r["trigger"]}
            for n, r in self.playbooks.registry().items()
            if not r.get("active")
        ]

    def adopt(self, draft: dict[str, Any]) -> dict[str, Any]:
        name, spec = draft["name"], draft["spec"]
        version = self.playbooks.next_version(name)
        path = self.playbooks.dir / f"{name}.v{version}.jev"
        source = str(draft.get("source") or "")
        path.write_text(source, encoding="utf-8")
        ok, errors = compile_check(path)
        drafted_by = "writer"
        if not ok:
            self.playbooks.audit("draft_rejected", name, version=version, errors=errors[:2000])
            path.write_text(render_playbook(spec, version), encoding="utf-8")
            ok, errors = compile_check(path)
            drafted_by = "template"
        if not ok:
            self.playbooks.audit("compile_failed", name, version=version, errors=errors[:2000])
            return {"name": name, "live": False, "reason": "did not compile"}
        verdict = self.verify(path, spec)
        self.playbooks.audit("verified", name, version=version, drafted_by=drafted_by, verdict=verdict)
        if verdict.get("ok"):
            self.playbooks.register(name, draft["category"], path, version, spec, verdict)
            self.host.memory.remember_learning(f"Playbook {name} v{version} encodes routine {draft['category'].replace('_', ' ')} work.")
            return {"name": name, "live": True, "version": version, "verdict": verdict}
        return {"name": name, "live": False, "version": version, "verdict": verdict}

    def verify(self, path: Path, spec: dict[str, Any]) -> dict[str, Any]:
        tasks = {t["id"]: t for t in self.host.ledger.records("task")}
        evidence = [tasks[i] for i in spec.get("evidence", []) if i in tasks]
        others = [t for t in tasks.values() if t["id"] not in spec.get("evidence", [])][-5:]
        rows = []
        program = self.host.session.load(path)
        try:
            for task, expected in [(t, True) for t in evidence] + [(t, False) for t in others]:
                replay_ok = False
                if expected:
                    final = replay(task.get("intake_recording", ""))
                    recorded = (final or {}).get("outputs", {}).get("result") or {}
                    if recorded.get("id") != task.get("request_id"):
                        recorded = None
                    replay_ok = bool(
                        recorded
                        and recorded.get("project") == task.get("project")
                        and recorded.get("kind") == task.get("kind")
                    )
                episode = self.host.episodes.run(
                    program,
                    "main",
                    {"request": {"text": task.get("request", ""), "project": task.get("project")}},
                    {},
                    subject=f"verify:{spec['name']}:{task['id']}",
                    output="plan",
                )
                plan = episode.result or {}
                rows.append(
                    {
                        "id": task["id"],
                        "expected_match": expected,
                        "matched": bool(plan.get("matches")),
                        "replay_ok": replay_ok,
                        "plan_project": plan.get("project"),
                        "expected_project": task.get("project"),
                        "plan_kind": plan.get("kind"),
                        "expected_kind": task.get("kind"),
                    }
                )
        finally:
            program.close()
        verdict = self.host.run_task("learn", {"phase": "prove", "results": rows, "policy": self.host.policy()}, subject=f"prove:{spec['name']}").result
        return {**(verdict or {"ok": False, "reasons": ["the verdict did not run"]}), "rows": rows}

    def run_playbook(self, name: str, request: dict[str, Any]) -> dict[str, Any] | None:
        path = self.playbooks.path_of(name)
        if path is None:
            return None
        program = self.host.session.load(path)
        try:
            episode = self.host.episodes.run(program, "main", {"request": {"text": request["text"], "project": request.get("effective_project") or request.get("project")}}, {}, subject=f"playbook:{name}:{request['id']}", output="plan")
        finally:
            program.close()
        plan = episode.result or {}
        if plan.get("matches"):
            self.playbooks.audit("applied", name, request=request["id"], recording=episode.recording)
        return plan
