"""The Jevscript modules compile, stay warning-free, and compose on their own.

Each module is a library a different harness can `use`; these tests load
small harness programs that import one module at a time from a search root
(spec section 3.9) and exercise its pure code with no model at all.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from conftest import JEV

from cos.fakejev import serving
from cos.jevbin import load
from cos.learning import render_playbook

MODULES = sorted(p.name for p in JEV.glob("*.jev"))


@pytest.mark.parametrize("module", MODULES)
def test_every_module_compiles_without_warnings(module: str, jevscript_bin: str) -> None:
    result = subprocess.run([jevscript_bin, "check", str(JEV / module)], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert result.stderr.strip() == "", result.stderr


def test_a_rendered_playbook_compiles(tmp_path: Path, jevscript_bin: str) -> None:
    spec = {
        "name": "pb_docs_change",
        "category": "docs_change",
        "project": "site",
        "projects": ["site", "site"],
        "kind": "ship",
        "effort": "low",
        "profile": "default",
        "obstacles": ['forgot to run "make docs" {twice}', "a \\ backslash"],
        "evidence": ["t1", "t2"],
    }
    path = tmp_path / "pb.jev"
    path.write_text(render_playbook(spec))
    result = subprocess.run([jevscript_bin, "check", str(path)], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr + path.read_text()


def harness(tmp_path: Path, source: str):
    path = tmp_path / "harness.jev"
    path.write_text(source)
    return load(str(path), paths=[str(JEV)])


def run_main(program, inputs, profiles=None, model=None, bind=None):
    run = program.task("main").start(inputs=inputs, profiles=profiles, model=model, bind=bind)
    pauses = list(run)
    assert pauses[-1]["kind"] == "done", pauses[-1]
    return pauses[-1]["outputs"]["answer"]


def test_routing_ready_honours_dependencies_holds_time_gates_and_capacity(tmp_path: Path, jevscript_bin: str) -> None:
    program = harness(
        tmp_path,
        """program ready_harness
use "routing.jev" as routing

in items: list
in now: number
in capacity: number
out answer

task main:
  answer = [i.id for i in routing.ready(items, now, capacity)]
""",
    )

    def item(i, status="queued", deps=(), hold=None):
        return {"id": i, "status": status, "deps": list(deps), "hold": hold}

    items = [
        item("done1", "done"),
        item("a", deps=["done1"]),
        item("b", deps=["a"]),  # a is not done yet
        item("c", hold={"reason": "owner", "until": None}),  # waits for the person
        item("d", hold={"reason": "later", "until": 100}),  # time gate passed at now=200
        item("e", hold={"reason": "later", "until": 500}),  # time gate not passed
        item("f"),
    ]
    try:
        run = program.task("main").start(inputs={"items": items, "now": 200, "capacity": 10})
        assert list(run)[-1]["outputs"]["answer"] == ["a", "d", "f"]
        run = program.task("main").start(inputs={"items": items, "now": 200, "capacity": 2})
        assert list(run)[-1]["outputs"]["answer"] == ["a", "d"]
    finally:
        program.close()


class _Nobody:
    """Bound but never called: the code under test makes no capability call."""

    def __init__(self, kind: str) -> None:
        self.kind = kind

    def call(self, verb, args, capability=None):  # pragma: no cover - must not happen
        raise AssertionError(f"unexpected call {verb}")


def test_playbook_verdict_is_code_only(tmp_path: Path, jevscript_bin: str) -> None:
    program = harness(
        tmp_path,
        """program verdict_harness
use "learn.jev" as learn with writer

in results: list
in policy: record
out answer

needs writer: llm

task main:
  answer = learn.verdict(results, policy)
""",
    )
    policy = {"min_evidence": 2, "min_recall": 1.0}

    def row(expected, matched=True, replay_ok=True, project="p", kind="ship"):
        return {"expected_match": expected, "matched": matched, "replay_ok": replay_ok, "plan_project": project, "expected_project": "p", "plan_kind": kind, "expected_kind": "ship"}

    try:
        good = [row(True), row(True), row(False, matched=False)]
        verdict = run_main(program, {"results": good, "policy": policy}, bind={"writer": _Nobody("llm")})
        assert verdict["ok"] is True and verdict["recall"] == 1

        false_hit = good + [row(False, matched=True)]
        verdict = run_main(program, {"results": false_hit, "policy": policy}, bind={"writer": _Nobody("llm")})
        assert verdict["ok"] is False and verdict["false_hits"] == 1

        bad_replay = [row(True), row(True, replay_ok=False)]
        verdict = run_main(program, {"results": bad_replay, "policy": policy}, bind={"writer": _Nobody("llm")})
        assert verdict["ok"] is False and verdict["replayed"] == 1

        wrong_route = [row(True), row(True, project="q")]
        verdict = run_main(program, {"results": wrong_route, "policy": policy}, bind={"writer": _Nobody("llm")})
        assert verdict["ok"] is False
    finally:
        program.close()


def test_escalation_channel_and_plain_words(tmp_path: Path, jevscript_bin: str) -> None:
    program = harness(
        tmp_path,
        """program channel_harness
use "escalate.jev" as escalation with me, writer, fleet

in cases: list
in text: text
out answer

needs me: person
needs writer: llm
needs fleet: tool

task main:
  answer = { channels: [escalation.channel(c[0], c[1]) for c in cases], plain: escalation.plain(text) }
""",
    )
    cases = [
        ["urgent", "away"],
        ["info", "normal"],
        ["decision", "away"],
        ["decision", "quiet"],
        ["outcome", "quiet"],
        ["outcome", "normal"],
    ]
    try:
        run = program.task("main").start(
            inputs={"cases": cases, "text": "The crewmate's worktree needs-decision before teardown"},
            bind={"me": _Nobody("person"), "writer": _Nobody("llm"), "fleet": _Nobody("tool")},
        )
        out = list(run)[-1]["outputs"]["answer"]
        assert out["channels"] == ["now", "digest", "digest", "now", "digest", "now"]
        assert out["plain"] == "The worker's isolated copy needs a decision before cleanup"
    finally:
        program.close()


def test_intake_assess_routes_by_judgment_and_code(tmp_path: Path, jevscript_bin: str, monkeypatch: pytest.MonkeyPatch) -> None:
    """Jev answers; the code in `intake.assess` decides the route."""
    monkeypatch.setenv("TYPESAFE_API_KEY", "offline")  # the fake endpoint ignores it
    program = harness(
        tmp_path,
        """program intake_harness
use "intake.jev" as intake
use "routing.jev" as routing

in req: record
in projects: list
in homes: list
in profiles: list
in policy: record
out answer

task main:
  answer = routing.route(req, intake.assess(req, projects, homes, profiles), policy)
""",
    )
    policy = {"min_kind_confidence": 0.35, "min_route_confidence": 0.45, "max_ambiguity": 0.8, "urgent": 0.8, "risk_hold": 0.7}
    inputs = {
        "projects": [{"name": "site", "description": "the website"}, {"name": "api", "description": "the api server"}],
        "homes": [{"name": "main", "scope": "anything"}, {"name": "ops", "scope": "infrastructure and deploys"}],
        "profiles": [{"name": "default", "rule": "anything"}],
        "policy": policy,
    }

    def assess(text, rules):
        with serving(rules) as fake:
            profiles = fake.write_profiles(tmp_path / f"p{abs(hash(text))}.json")
            return run_main(program, {**inputs, "req": {"id": "r1", "text": text}}, profiles=str(profiles), model="cos-offline"), fake

    try:
        decided, fake = assess(
            "Bump the api's serde pin",
            [{"match": {"id": "^work$"}, "answer": {"choice": "ship"}}, {"match": {"id": "^project$"}, "answer": {"item": "api"}}, {"match": {"id": "^home$"}, "answer": {"choice": "i0"}}, {"match": {"id": "^effort$"}, "answer": {"score": 0}}],
        )
        assert decided["route"] == "backlog" and decided["project"] == "api" and decided["effort"] == "low"
        assert len(fake.requests) == 1, "every intake question rides one request"
        assert set(fake.requests[0]["state"]) == {"text", "projects", "homes", "profiles"}, "state is exactly the subjects"

        decided, _ = assess("Always use conventional commits", [{"match": {"id": "^work$"}, "answer": {"choice": "memory"}}])
        assert decided["route"] == "memory"

        decided, _ = assess("Do the thing", [{"match": {"id": "^work$"}, "answer": {"choice": "ship"}}, {"match": {"id": "^project$"}, "answer": {"choice": "none"}}, {"match": {"id": "^home$"}, "answer": {"choice": "i0"}}])
        assert decided["route"] == "ask" and "which project" in decided["reason"]

        decided, _ = assess("Rotate the prod database password", [{"match": {"id": "^work$"}, "answer": {"choice": "ship"}}, {"match": {"id": "^project$"}, "answer": {"choice": "i1"}}, {"match": {"id": "^home$"}, "answer": {"choice": "i0"}}, {"match": {"id": "^risky$"}, "answer": {"noul": 0.95}}])
        assert decided["route"] == "backlog" and decided["hold"] is True

        decided, _ = assess("Redeploy the staging cluster", [{"match": {"id": "^work$"}, "answer": {"choice": "ship"}}, {"match": {"id": "^project$"}, "answer": {"choice": "i1"}}, {"match": {"id": "^home$"}, "answer": {"choice": "i1"}}])
        assert decided["route"] == "mate" and decided["mate"] == "ops"
    finally:
        program.close()
