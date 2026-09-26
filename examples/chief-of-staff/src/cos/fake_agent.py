"""A fake `agent` adapter process speaking the JSONL adapter protocol.

``python -m cos.fake_agent`` stands in for a real agent adapter in
tests and in ``cos demo``: it implements ``spawn``, ``observe``, ``send``,
``wait`` and ``stop`` (spec section 9.1) and "works" by following a script.
It is a real process behind the same boundary a real adapter uses, so the
host code under test is exactly the code that binds Claude Code or Codex.

Each worker reads its instructions (the brief path in the spawn prompt) to
learn its status file, inbox, branch and delivery, then advances one step per
observation. Steps: ``work`` (print progress), ``commit`` (commit a file in
its isolated copy), ``done`` (report ready as the brief asks), ``approve`` and
``request_changes:<text>`` (a reviewer's verdict), ``stall``
(freeze until nudged), ``ask:<text>`` (report needs-decision, then wait for
an answer), ``fail:<text>``, ``die`` (the session exits), ``loop`` (print the
same error again and again) and ``idle``.

A script is chosen by the first ``COS_FAKE_AGENT_SCRIPTS`` rule (a JSON file
of ``[{"match": regex, "steps": [...]}]``) whose regex finds the task text;
the default is ``work, commit, done`` (``work, approve`` for a reviewer, whose
task text is matched with a ``review: `` prefix). State persists in
``COS_FAKE_AGENT_STATE`` so a restarted adapter reattaches by name.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

DEFAULT_STEPS = ["work", "commit", "done"]


def _brief_fields(text: str) -> dict[str, str]:
    def grab(pattern: str) -> str:
        match = re.search(pattern, text)
        return match.group(1).rstrip(".:") if match else ""

    return {
        "status": grab(r"appending one line to (\S+?):?\n") or grab(r"append exactly one line to (\S+?):\n"),
        "inbox": grab(r"Instructions may arrive in (\S+?)\. "),
        "branch": grab(r"on branch (\S+?)\."),
        "report": grab(r"report at (\S+?\.md)"),
        "task": text.split("# Task", 1)[-1].split("\n\n", 1)[0].strip() if "# Task" in text else text[:200],
        "delivery": "pr" if "pull request" in text and "Never push" not in text else ("report" if "report at" in text else "local"),
    }


class FakeAgents:
    def __init__(self, state_path: str | None, scripts_path: str | None) -> None:
        self.state_path = Path(state_path) if state_path else None
        self.scripts = json.loads(Path(scripts_path).read_text()) if scripts_path else []
        self.workers: dict[str, dict[str, Any]] = {}
        if self.state_path and self.state_path.exists():
            self.workers = json.loads(self.state_path.read_text())

    def save(self) -> None:
        if self.state_path:
            tmp = self.state_path.with_suffix(".tmp")
            tmp.write_text(json.dumps(self.workers))
            os.replace(tmp, self.state_path)

    def steps_for(self, task: str) -> list[str]:
        for rule in self.scripts:
            if re.search(rule["match"], task, re.IGNORECASE):
                return list(rule["steps"])
        return list(DEFAULT_STEPS)

    # -- verbs ------------------------------------------------------------------

    def spawn(self, named: dict[str, Any]) -> dict[str, Any]:
        name = str(named.get("name") or f"w{len(self.workers) + 1}")
        where = named.get("in") or {}
        cwd = where.get("path") or where.get("cwd") or named.get("cwd") or os.getcwd()
        prompt = str(named.get("prompt", ""))
        existing = self.workers.get(name)
        if existing and existing["alive"]:
            return self.handle(existing)
        brief_path = re.search(r"Read (\S+?) (and|again)", prompt)
        brief = Path(brief_path.group(1)).read_text() if brief_path and Path(brief_path.group(1)).exists() else prompt
        fields = _brief_fields(brief)
        reviewer = named.get("role") == "reviewer" or brief.startswith("# Review")
        steps = self.steps_for(("review: " if reviewer else "") + fields["task"]) if not reviewer or self.scripts else ["work", "approve"]
        if reviewer and steps == DEFAULT_STEPS:
            steps = ["work", "approve"]
        worker = existing or {"name": name, "step": 0, "steps": steps, "screen": [], "sent": [], "launches": 0}
        worker.update(alive=True, cwd=cwd, fields=fields, harness=named.get("harness"), model=named.get("model"), effort=named.get("effort"), waiting=None)
        worker["launches"] += 1
        worker["screen"].append(f"[{named.get('harness') or 'agent'}] started in {cwd}")
        self.workers[name] = worker
        return self.handle(worker)

    def handle(self, worker: dict[str, Any]) -> dict[str, Any]:
        return {"id": worker["name"], "name": worker["name"], "cwd": worker["cwd"], "launch": worker["launches"]}

    def status(self, worker: dict[str, Any], kind: str, text: str) -> None:
        path = worker["fields"].get("status")
        if path:
            Path(path).parent.mkdir(parents=True, exist_ok=True)
            with open(path, "a", encoding="utf-8") as handle:
                handle.write(f"{kind} [at={int(time.time())}]: {text}\n")

    def advance(self, worker: dict[str, Any]) -> None:
        if not worker["alive"] or worker.get("waiting"):
            return
        if worker["step"] >= len(worker["steps"]):
            return
        step = worker["steps"][worker["step"]]
        worker["step"] += 1
        head, _, arg = step.partition(":")
        fields = worker["fields"]
        if head == "work":
            worker["screen"].append(f"working on: {fields['task'][:60]} (step {worker['step']})")
            if worker["step"] == 1:
                self.status(worker, "working", "started")
        elif head == "commit":
            target = Path(worker["cwd"]) / f"cos-{worker['name']}.txt"
            target.write_text(fields["task"] + "\n")
            subprocess.run(["git", "-C", worker["cwd"], "add", target.name], capture_output=True, check=False)
            subprocess.run(["git", "-C", worker["cwd"], "-c", "user.name=cos-fake", "-c", "user.email=cos-fake@example.invalid", "commit", "-qm", f"fake: {fields['task'][:50]}"], capture_output=True, check=False)
            worker["screen"].append("committed the change")
        elif head == "done":
            if fields["delivery"] == "report":
                Path(fields["report"]).parent.mkdir(parents=True, exist_ok=True)
                Path(fields["report"]).write_text(f"# Report\n\n{fields['task']}\n\nFinding: nothing surprising.\n")
                self.status(worker, "done", "report ready")
            elif fields["delivery"] == "pr":
                self.status(worker, "done", f"PR {arg or 'https://github.com/example/repo/pull/1'}")
            else:
                self.status(worker, "done", f"ready in branch {fields['branch']}")
            worker["screen"].append("done; waiting for input")
        elif head == "approve":
            self.status(worker, "done", "approved - the change does what the task asks")
            worker["screen"].append("review finished: approved")
        elif head == "request_changes":
            self.status(worker, "blocked", f"changes requested - {arg or 'add a test'}")
            worker["screen"].append("review finished: changes requested")
        elif head == "stall":
            worker["waiting"] = "nudge"
        elif head == "ask":
            self.status(worker, "needs-decision", arg or "which option should I take?")
            worker["waiting"] = "answer"
        elif head == "fail":
            self.status(worker, "failed", arg or "could not finish")
        elif head == "die":
            worker["alive"] = False
        elif head == "loop":
            worker["screen"].append(f"error: the same test failed again; retrying the same fix ({worker['step']})")
            if not worker.get("nudged"):
                worker["step"] -= 1

    def observe(self, handle: dict[str, Any]) -> dict[str, Any]:
        worker = self.workers.get(str(handle.get("id")))
        if worker is None:
            return {"status": "exited", "last_message": "", "tail": "", "exit_code": None}
        self.advance(worker)
        tail = "\n".join(worker["screen"][-40:])
        status = "exited" if not worker["alive"] else ("waiting" if worker.get("waiting") or worker["step"] >= len(worker["steps"]) else "running")
        return {"status": status, "last_message": worker["screen"][-1] if worker["screen"] else "", "tail": tail, "exit_code": None if worker["alive"] else 1, "busy": status == "running"}

    def send(self, handle: dict[str, Any], text: str) -> None:
        worker = self.workers[str(handle.get("id"))]
        worker["sent"].append(text)
        worker["screen"].append(f"> {text[:120]}")
        inbox = worker["fields"].get("inbox")
        if "Instruction waiting" in text and inbox:
            handled = Path(inbox) / "handled"
            handled.mkdir(parents=True, exist_ok=True)
            for message in sorted(Path(inbox).glob("*.msg")):
                body = message.read_text()
                worker["screen"].append(f"read instruction: {body.split('--', 1)[-1].strip()[:80]}")
                os.replace(message, handled / message.name)
                if worker.get("waiting"):
                    worker["waiting"] = None
                    worker["nudged"] = True
        elif worker.get("waiting"):
            worker["waiting"] = None

    def stop(self, handle: dict[str, Any]) -> None:
        worker = self.workers.get(str(handle.get("id")))
        if worker:
            worker["alive"] = False

    # -- protocol ---------------------------------------------------------------

    def serve_one(self, request: dict[str, Any]) -> dict[str, Any]:
        if request.get("operation") == "observe":
            return {"observation": self.observe(request.get("handle") or {})}
        verb = request.get("verb")
        args = request.get("args") or {}
        positional = args.get("positional") or []
        named = args.get("named") or {}
        if verb == "spawn":
            return {"result": {**self.spawn(named)}}
        if verb in ("observe", "wait"):
            return {"result": self.observe(positional[0] if positional else named.get("handle", {}))}
        if verb == "send":
            self.send(positional[0], str(positional[1] if len(positional) > 1 else named.get("text", "")))
            return {"result": None}
        if verb == "stop":
            self.stop(positional[0] if positional else named.get("handle", {}))
            return {"result": None}
        return {"error": {"message": f"fake agent has no verb `{verb}`", "retryable": False}}


def main() -> None:
    agents = FakeAgents(os.environ.get("COS_FAKE_AGENT_STATE"), os.environ.get("COS_FAKE_AGENT_SCRIPTS"))
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            reply = agents.serve_one(json.loads(line))
        except Exception as error:  # noqa: BLE001 - every failure is a protocol error reply
            reply = {"error": {"message": f"{type(error).__name__}: {error}", "retryable": False}}
        agents.save()
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
