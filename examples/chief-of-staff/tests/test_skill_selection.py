"""Skill selection at catalog scale (S6) through the real host, runtime and recordings.

The scout is the built-in ``HarnessScout`` running a fake ``claude`` or
``codex`` executable on PATH, so the model binding, the prompt and the
three-line contract cross a real process boundary. Jev is the offline fake;
its rules mark which shortlisted Skills fit.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from conftest import run_until
from cos.state.home import DEFAULT_CONFIG, read_jsonl
from skill_fixture import LARGE_RELEVANT, LARGE_SCOUT, LARGE_TASK, catalog_texts, distractors, write_catalog, write_skill


@pytest.fixture
def harness(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """Put fake `claude` and `codex` CLIs on PATH; return (set rules, read calls)."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    source = (Path(__file__).parent / "fake_harness.py").read_text()
    for name in ("claude", "codex"):
        path = bin_dir / name
        path.write_text(f"#!{sys.executable}\n{source}")
        path.chmod(0o755)
    log, rules = tmp_path / "harness.jsonl", tmp_path / "harness-rules.json"
    rules.write_text("[]")
    monkeypatch.setenv("PATH", f"{bin_dir}:{Path(sys.executable).parent}:/usr/bin:/bin")
    monkeypatch.setenv("COS_FAKE_HARNESS_LOG", str(log))
    monkeypatch.setenv("COS_FAKE_HARNESS_RULES", str(rules))

    def script(*pairs: tuple[str, str]) -> None:
        rules.write_text(json.dumps([{"match": match, "text": text} for match, text in pairs]))

    return script, lambda: read_jsonl(log)


def fit_ids(ids: list[str]) -> list[dict[str, Any]]:
    """Fake Jev: a shortlisted Skill fits when its record names one of ``ids``."""
    pattern = "|".join(ids)
    return [{"match": {"id": r"^fits\[", "text": rf'"id": "({pattern})"'}, "answer": {"noul": 0.94}}]


def dispatch_events(worker: dict[str, Any]) -> list[dict[str, Any]]:
    return read_jsonl(Path(worker["dispatch_recording"]))


def calls(events: list[dict[str, Any]], verb: str) -> list[dict[str, Any]]:
    return [e for e in events if e.get("event") == "call" and e.get("verb") == verb]


def fit_requests(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [e for e in events if e.get("event") == "request" and any(q["id"].startswith("fits[") for q in e["questions"])]


def test_large_catalog_scouts_once_searches_once_and_judges_one_shortlist(make_host, harness, tmp_path: Path, jevscript_bin: str) -> None:
    """S6 acceptance: 1,000 approved Skills, 5 relevant; one Haiku scout
    generation, one recorded search holding all 5, one Jev request over at
    most 32 Skills, then the worker starts with exactly those Skills."""
    script, harness_calls = harness
    script(("Upgrade tokio", LARGE_SCOUT))
    texts = catalog_texts()
    # A distinct Skill whose text the query cannot tell from rust-deps: both reach Jev.
    texts["rust-deps-workspace"] = texts["rust-deps"]
    catalog = write_catalog(tmp_path / "catalog", texts)
    assert len(catalog) == 1001
    relevant = LARGE_RELEVANT + ["rust-deps-workspace"]
    host = make_host(rules=fit_ids(relevant), yolo=False, config={"skill_catalog": catalog})
    host.submit(LARGE_TASK)
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    events = dispatch_events(worker)

    generations = [e for e in events if e.get("event") == "generate"]
    scouts = [e for e in generations if e["capability"] == "scout"]
    assert len(scouts) == 1 and scouts[0]["using"]["model"] == "claude-haiku-4-5-20251001" and scouts[0]["output"] == LARGE_SCOUT + "\n"
    [scout_call] = harness_calls()
    assert scout_call["cli"] == "claude" and scout_call["argv"][scout_call["argv"].index("--model") + 1] == "claude-haiku-4-5-20251001"

    [search] = calls(events, "skill_search")
    found = search["result"]
    assert set(relevant) <= set(found["ids"])
    assert (found["scanned"], found["kept"], found["filtered_out"], found["fallback"]) == (1001, 32, 969, False)
    assert found["skills"][0].keys() == {"id", "description", "dependencies"}

    [judged] = fit_requests(events)
    assert len([q for q in judged["questions"] if q["id"].startswith("fits[")]) == 32
    assert sorted(s["id"] for s in worker["skills"]) == sorted(relevant)
    assert worker["skill_search"]["filtered_out"] == 969
    brief = Path(worker["brief"]).read_text()
    assert "judged 32 of 1001 approved Skills; 969 were left out" in brief
    assert not any(e.get("event") == "pause" and e.get("kind") == "budget" for e in events)


def test_replay_serves_the_recorded_shortlist_without_scout_or_search(make_host, harness, tmp_path: Path, jevscript_bin: str) -> None:
    """§10.4: replaying the dispatch recording asks neither the scout nor the
    host's search again; the recorded generation and search result are served."""
    script, harness_calls = harness
    script(("Upgrade tokio", LARGE_SCOUT))
    host = make_host(rules=fit_ids(LARGE_RELEVANT), yolo=False, config={"skill_catalog": write_catalog(tmp_path / "catalog", catalog_texts())})
    host.submit(LARGE_TASK)
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    recorded = calls(dispatch_events(worker), "skill_search")[0]["result"]
    host.close()
    shutil.rmtree(tmp_path / "catalog")  # a replay that searched again could not find a single Skill
    replay = subprocess.run([jevscript_bin, "replay", worker["dispatch_recording"]], capture_output=True, text=True, check=False)
    assert replay.returncode == 0, replay.stderr
    assert len(harness_calls()) == 1
    done = json.loads(replay.stdout.strip().splitlines()[-1])
    [started] = done["outputs"]["result"]["started"]
    assert done["kind"] == "done" and started["skill_search"] == worker["skill_search"]
    assert [s["id"] for s in started["skills"]] == [s["id"] for s in worker["skills"]] and set(LARGE_RELEVANT) <= set(recorded["ids"])


def test_codex_items_scout_with_luna(make_host, harness, tmp_path: Path) -> None:
    script, harness_calls = harness
    script(("Upgrade tokio", LARGE_SCOUT))
    host = make_host(rules=fit_ids(LARGE_RELEVANT), yolo=False, config={"skill_catalog": write_catalog(tmp_path / "catalog", catalog_texts())})
    host.registry.set_profiles([{"name": "default", "rule": "any work", "harness": "codex"}])
    host.submit(LARGE_TASK)
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    [scout] = [e for e in dispatch_events(worker) if e.get("event") == "generate" and e["capability"] == "scout"]
    assert scout["using"]["harness"] == "codex" and scout["using"]["model"] == "gpt-6-luna"
    [cli] = harness_calls()
    assert cli["cli"] == "codex" and cli["argv"][cli["argv"].index("-m") + 1] == "gpt-6-luna"
    assert set(LARGE_RELEVANT) <= {s["id"] for s in worker["skills"]}


@pytest.mark.parametrize(("harness_name", "reply", "reason"), [
    ("claude", "Here are some terms: tokio, cargo", "scout output rejected: expected 3 lines, got 1"),
    ("claude", "kind: rust\n\nterms: tokio\n\nideal_skill: Fix Rust dependencies.", "scout output rejected: expected 3 lines, got 5"),
    ("aider", None, "no scout model for this harness"),
    ("claude", None, "no scout model for this harness"),
])
def test_scout_fallback_is_recorded_and_dispatch_continues(make_host, harness, tmp_path: Path, harness_name: str, reply: str | None, reason: str) -> None:
    """A reply outside the three-line schema, or a harness with no scout model,
    searches with the request's own words and records why."""
    script, harness_calls = harness
    if reply is not None:
        script(("Upgrade tokio", reply))
    config = {"skill_catalog": write_catalog(tmp_path / "catalog", catalog_texts())}
    if harness_name == "claude" and reply is None:
        config["scout"] = {"models": {"claude": None}}
    host = make_host(rules=[], yolo=False, config=config)
    host.registry.set_profiles([{"name": "default", "rule": "any work", "harness": harness_name}])
    host.submit(LARGE_TASK)
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    events = dispatch_events(worker)
    [search] = calls(events, "skill_search")
    assert search["result"]["fallback"] is True and search["result"]["fallback_reason"] == reason
    assert search["result"]["kept"] == 32 and worker["skill_search"]["fallback"] is True
    assert search["result"]["scouted"] is (reply is not None)
    assert len(harness_calls()) == (1 if reply is not None else 0)
    if reply is None:
        assert any(e.get("event") == "log" and "no scout model" in json.dumps(e) for e in events)


def test_larger_shortlist_shrinks_the_batch_instead_of_pausing(make_host, harness, tmp_path: Path) -> None:
    """With k = 128 each item nominally costs writer + scout + 2 Jev chunks,
    so a 15-item wake starts 12 (48 calls) and leaves 3 for the next wake."""
    script, _ = harness
    script(("", "kind: docs\nterms: readme, typo\nideal_skill: Use when fixing README typos."))
    catalog = write_catalog(tmp_path / "catalog", {f"docs-{index:03d}": "Use when fixing README typos in documentation." for index in range(200)})
    host = make_host(rules=[], yolo=False, config={"skill_catalog": catalog, "policy": {**DEFAULT_CONFIG["policy"], "skill_shortlist": 128}})
    for index in range(15):
        host.backlog.add({"text": f"Fix typo number {index} in the README", "title": f"typo {index}", "project": "proj", "kind": "ship", "effort": "low", "profile": "default"})
    host.wakes.push("dispatch", "backlog", ["test"])
    first, second = host.tick()["handled"]
    assert (len(first["result"]["started"]), first["result"]["planned_calls"], first["result"]["more_ready"]) == (12, 48, True)
    assert (len(second["result"]["started"]), second["result"]["planned_calls"], second["result"]["more_ready"]) == (3, 12, False)
    episodes = [r for r in host.ledger.records("episode") if r["task"] == "on_wake"]
    assert [r["problem"] for r in episodes] == [None, None] and all(r["calls"] <= 48 for r in episodes)
    assert not [d for d in host.decisions.open() if "budget" in d["question"]]
    assert len([w for w in host.workers.all(live_only=False) if w.get("handle")]) == 15


def test_pinned_and_playbook_skills_bypass_the_filter(make_host, harness, tmp_path: Path) -> None:
    script, _ = harness
    script(("Upgrade tokio", LARGE_SCOUT))
    texts = catalog_texts()
    catalog = write_catalog(tmp_path / "catalog", texts)
    by_id = {entry["id"]: entry for entry in catalog}
    by_id["cobol-onboarding-guides"]["pinned"] = True
    by_id["figma-export"]["playbooks"] = ["pb_dependency_update"]
    host = make_host(rules=fit_ids(LARGE_RELEVANT), yolo=False, config={"skill_catalog": catalog})
    item = host.backlog.add({"text": LARGE_TASK, "title": "tokio", "project": "proj", "kind": "ship", "effort": "low", "profile": "default", "playbook": "pb_dependency_update"})
    host.wakes.push("dispatch", "backlog", ["test"])
    run_until(host, lambda: bool(host.workers.get(item["id"])) and bool(host.workers.get(item["id"]).get("handle")))
    [search] = calls(dispatch_events(host.workers.get(item["id"])), "skill_search")
    found = search["result"]
    assert found["always"] == ["figma-export", "cobol-onboarding-guides"]
    assert found["ids"][:2] == found["always"] and len(found["ids"]) == 32
    assert set(LARGE_RELEVANT) <= set(found["ids"])


def test_an_item_whose_always_list_cannot_fit_is_held_with_the_count_and_ids(make_host, harness, tmp_path: Path) -> None:
    """449 pinned Skills need ceil(449/64) = 8 Jev chunks plus the writer: 9
    calls, over the 8 one dispatch allows. The item is held, nothing is dropped."""
    script, harness_calls = harness
    catalog = [write_skill(tmp_path / "catalog", skill_id, text, pinned=True) for skill_id, text in list(distractors().items())[:449]]
    host = make_host(rules=[], yolo=False, config={"skill_catalog": catalog})
    host.submit("Fix the README typo")
    run_until(host, lambda: any(":skills" in d["key"] for d in host.decisions.open()))
    [decision] = [d for d in host.decisions.open() if d["key"].endswith(":skills")]
    assert "would need 9 model calls, over the 8 one dispatch allows: 449 Skills to judge, 449 of them always included" in decision["question"]
    assert catalog[0]["id"] in decision["question"] and catalog[-1]["id"] in decision["question"]
    assert not host.workers.all(live_only=False) and not harness_calls()
    assert host.session.fake is not None and not [r for r in host.session.fake.requests if any(q.startswith("fits[") for q in r["questions"])]


def test_overlong_description_is_rejected_at_catalog_validation(make_host, tmp_path: Path) -> None:
    src = Path(__file__).resolve().parents[1] / "src"
    home = tmp_path / "cli-home"
    env = {**os.environ, "PYTHONPATH": str(src)}

    def set_catalog(catalog: list[dict[str, Any]]) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, "-m", "cos", "--home", str(home), "config", "set", "skill_catalog", json.dumps(catalog)], env=env, capture_output=True, text=True, check=False)

    valid = [write_skill(tmp_path / "catalog", "valid", "A valid Skill.")]
    assert set_catalog(valid).returncode == 0
    before = (home / "config.json").read_bytes()
    invalid = [write_skill(tmp_path / "catalog", "verbose", "x" * 1025)]
    rejected = set_catalog(invalid)
    assert rejected.returncode != 0 and "skill verbose: description exceeds 1024 characters" in rejected.stderr
    assert (home / "config.json").read_bytes() == before


def test_sparse_large_shortlist_uses_recorded_cardinality(make_host, harness, tmp_path: Path) -> None:
    script, _ = harness
    script(("", "kind: needle repair\nterms: needle\nideal_skill: Fix a needle failure."))
    texts = {f"archive-{index:04d}": "Use for astronomy observations." for index in range(990)}
    texts.update({f"needle-{index}": "Use when repairing a needle failure." for index in range(10)})
    host = make_host(rules=fit_ids(["needle-0"]), yolo=False, config={"skill_catalog": write_catalog(tmp_path / "catalog", texts), "policy": {**DEFAULT_CONFIG["policy"], "skill_shortlist": 512}})
    host.submit("Repair the needle failure")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    events = dispatch_events(worker)
    [found] = calls(events, "skill_search")
    assert found["result"]["kept"] == 10 and found["result"]["scanned"] == 1000
    assert len([e for e in events if e.get("event") == "generate" and e["capability"] == "scout"]) == 1
    assert len(fit_requests(events)) == 1 and worker["handle"]
    assert not [d for d in host.decisions.open() if d["key"].endswith(":skills")]


def test_long_dependency_chain_installs_through_recorded_dispatch(make_host, harness, tmp_path: Path) -> None:
    script, _ = harness
    script(("", "kind: needle repair\nterms: needle\nideal_skill: Repair the needle failure."))
    catalog = [write_skill(tmp_path / "catalog", f"step-{index:04d}", "Use for a generic chain task." if index < 1049 else "Use when repairing the needle failure.") for index in range(1050)]
    for index in range(1, len(catalog)):
        catalog[index]["dependencies"] = [catalog[index - 1]["id"]]
    host = make_host(rules=fit_ids([catalog[-1]["id"]]), yolo=False, config={"skill_catalog": catalog})
    host.submit("Repair the needle failure")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    assert len(worker["skills"]) == len(catalog)
    assert worker["skills"][0]["id"] == catalog[0]["id"]
    assert worker["skills"][-1]["id"] == catalog[-1]["id"]


def test_retry_brief_uses_receipted_uncertainty_after_shortlist_changes(make_host, harness, tmp_path: Path) -> None:
    script, _ = harness
    script(("", "kind: docs\nterms: markdown\nideal_skill: Write Markdown docs."))
    catalog = [write_skill(tmp_path / "catalog", "docs", "Write Markdown docs."), write_skill(tmp_path / "catalog", "tests", "Fix pytest tests.")]
    config = {"skill_catalog": catalog, "policy": {**DEFAULT_CONFIG["policy"], "skill_shortlist": 1}}
    uncertain_fit = [{"match": {"id": r"^fits\["}, "answer": {"noul": 0.5}}]
    host = make_host(rules=uncertain_fit, yolo=False, config=config)
    host.submit("Handle this item")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    assert "docs: Write Markdown docs." in Path(worker["brief"]).read_text()
    host.workers.update(worker["id"], handle=None)
    host.backlog.update(worker["id"], status="in_flight")
    host.close()
    script(("", "kind: testing\nterms: pytest\nideal_skill: Fix pytest tests."))
    restarted = make_host(rules=uncertain_fit, yolo=False, config=config, home_dir=tmp_path / "home", register=False)
    restarted.recover()
    restarted.wakes.push("dispatch", "backlog", ["recovered"])
    run_until(restarted, lambda: bool(restarted.workers.get(worker["id"]).get("handle")))
    recovered = restarted.workers.get(worker["id"])
    [found] = calls(dispatch_events(recovered), "skill_search")
    assert found["result"]["ids"] == ["tests"]
    assert "docs: Write Markdown docs." in Path(recovered["brief"]).read_text()


def test_cos_tick_through_the_cli_selects_from_a_large_catalog(harness, tmp_path: Path, project: Path, jevscript_bin: str) -> None:
    """The same acceptance scenario driven only through the `cos` command line."""
    script, harness_calls = harness
    script(("Upgrade tokio", LARGE_SCOUT))
    src = Path(__file__).resolve().parents[1] / "src"
    rules = tmp_path / "rules.json"
    rules.write_text(json.dumps(fit_ids(LARGE_RELEVANT) + [
        {"match": {"id": "^work$"}, "answer": {"choice": "ship"}},
        {"match": {"id": "^project$"}, "answer": {"choice": "i0"}},
        {"match": {"id": "^home$"}, "answer": {"choice": "i0"}},
    ]))
    catalog = tmp_path / "catalog.json"
    catalog.write_text(json.dumps(write_catalog(tmp_path / "catalog", catalog_texts())))
    home = tmp_path / "cli-home"
    env = {**os.environ, "JEVSCRIPT_BIN": jevscript_bin, "COS_JEV": "fake", "PYTHONPATH": f"{src}:{Path(__file__).resolve().parents[3] / 'sdk' / 'python' / 'src'}"}

    def cos(*args: str) -> str:
        run = subprocess.run([sys.executable, "-m", "cos", "--home", str(home), *args], env=env, capture_output=True, text=True, check=False)
        assert run.returncode == 0, run.stderr
        return run.stdout

    cos("init")
    cos("config", "set", "skill_catalog", catalog.read_text())
    cos("config", "set", "jev.fake_rules", str(rules))
    cos("config", "set", "adapters.crew", json.dumps({"command": [sys.executable, "-m", "cos.fake_agent"], "env": {"COS_FAKE_AGENT_STATE": str(tmp_path / "agents.json"), "PYTHONPATH": str(src)}}))
    cos("project", "add", "proj", str(project), "--mode", "local-only")
    cos("say", LARGE_TASK, "--project", "proj")
    cos("tick")
    [worker] = [json.loads(path.read_text()) for path in (home / "state" / "workers").glob("*.json")]
    assert worker.get("handle") and sorted(s["id"] for s in worker["skills"]) == sorted(LARGE_RELEVANT)
    events = dispatch_events(worker)
    assert len([e for e in events if e.get("event") == "generate" and e["capability"] == "scout"]) == 1
    assert calls(events, "skill_search")[0]["result"]["filtered_out"] == 968
    assert len(fit_requests(events)) == 1 and len(harness_calls()) == 1
