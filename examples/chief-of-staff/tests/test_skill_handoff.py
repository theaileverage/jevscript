"""Task Skill selection through the real CoS host, Jev runtime and agent handoff.

The fake Jev supplies labelled judgments; the policy, host effects, recording,
filesystem and worker adapter are the same paths used by the CLI.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

import pytest

from conftest import git, run_until

from cos.state.home import read_jsonl, write_json
from cos.skills import SkillError


def approved_skills(tmp_path: Path, names: list[str], dependencies: dict[str, list[str]] | None = None) -> list[dict[str, Any]]:
    entries = []
    for name in names:
        source = tmp_path / "approved" / name / "SKILL.md"
        source.parent.mkdir(parents=True)
        source.write_text(f"---\nname: {name}\ndescription: Follow the {name} procedure for {name} work.\n---\n\n# {name}\n\nDo {name} work.\n")
        entries.append({"id": name, "path": str(source), "sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "dependencies": (dependencies or {}).get(name, [])})
    return entries


def add_support(entry: dict[str, Any], relative: str, content: bytes, mode: int = 0o644) -> None:
    path = Path(entry["path"]).parent / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    path.chmod(mode)
    entry.setdefault("files", {"SKILL.md": entry["sha256"]})[relative] = hashlib.sha256(content).hexdigest()


def fits(*selected: str, uncertain: tuple[str, ...] = ()) -> list[dict[str, Any]]:
    """Fake Jev fits by Skill id: the shortlist is in the host's ranked order, not catalog order."""
    judged = [(skill_id, 0.94) for skill_id in selected] + [(skill_id, 0.5) for skill_id in uncertain]
    return [{"match": {"id": r"^fits\[", "text": rf'"id": "{skill_id}"'}, "answer": {"noul": fit}} for skill_id, fit in judged] + [
        {"match": {"id": r"^fits\["}, "answer": {"noul": 0.1}}
    ]


@pytest.mark.parametrize(
    ("task_text", "chosen", "expected"),
    [
        ("Explain what documentation tests are", [], []),
        ("Update the docs", ["docs"], ["docs"]),
        ("Update the docs and run the tests", ["docs", "tests"], ["docs", "base", "tests"]),
    ],
)
def test_host_selects_zero_one_or_many_skills_before_worker_start(make_host, tmp_path: Path, task_text: str, chosen: list[str], expected: list[str]) -> None:
    """§6.5/9.6: independent fits retain both task facets, then the host
    adds prerequisites and hands verified project paths to the worker."""
    catalog = approved_skills(tmp_path, ["docs", "base", "tests", "unrelated"], {"tests": ["base"]})
    host = make_host(rules=fits(*chosen), yolo=False, config={"skill_catalog": catalog})
    host.submit(task_text)
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    [worker] = host.workers.all()
    selected = worker["skills"]
    assert [entry["id"] for entry in selected] == expected
    brief = Path(worker["brief"]).read_text()
    for entry in selected:
        path = Path(entry["path"])
        assert path.is_file()
        assert hashlib.sha256(path.read_bytes()).hexdigest() == entry["sha256"]
        assert entry["id"] in brief and entry["path"] in brief and entry["sha256"] in brief
    if not expected:
        assert "No approved task Skills matched" in brief
    events = read_jsonl(Path(worker["dispatch_recording"]))
    verbs = [event.get("verb") for event in events if event.get("event") == "call"]
    assert verbs.index("worktree") < verbs.index("ensure_skills") < verbs.index("write_brief") < verbs.index("spawn")
    assert host.session.fake is not None
    assert any(any(key.startswith("fits[") for key in request["questions"]) for request in host.session.fake.requests)
    assert all(catalog[0]["path"] not in json.dumps(request) for request in host.session.fake.requests)


def test_uncertain_selection_keeps_confident_skills_and_names_the_open_question(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs", "tests"])
    host = make_host(rules=fits("docs", uncertain=("tests",)), yolo=False, config={"skill_catalog": catalog})
    host.submit("Update docs and tests")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    [worker] = host.workers.all()
    assert [skill["id"] for skill in worker["skills"]] == ["docs"] and worker["skill_selection"] == "uncertain"
    brief = Path(worker["brief"]).read_text()
    assert worker["skills"][0]["path"] in brief
    assert "Selection is uncertain" in brief and "tests: Follow the tests procedure" in brief


def test_collision_or_changed_source_prevents_spawn(make_host, tmp_path: Path, project: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    colliding = project / ".agents" / "skills" / "docs" / "SKILL.md"
    colliding.parent.mkdir(parents=True)
    colliding.write_text("unapproved contents\n")
    git(project, "add", ".")
    git(project, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "existing skill")
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    host.submit("Update docs")
    host.tick()
    workers = host.workers.all(live_only=False)
    assert workers and not workers[0].get("handle")
    assert not (host.home.task_dir(workers[0]["id"]) / "brief.md").exists()
    assert "skill destination collision" in " ".join(d["question"] for d in host.decisions.open())
    assert colliding.read_text() == "unapproved contents\n"


def test_selected_skill_lands_and_cleanup_does_not_commit_host_files(make_host, tmp_path: Path, project: Path) -> None:
    """§9.6: verified host files do not make normal git landing dirty."""
    catalog = approved_skills(tmp_path, ["docs"])
    host = make_host(rules=fits("docs"), yolo=True, config={"skill_catalog": catalog})
    remove_owned = host.skills.remove_owned

    def after_agent_stop(task_id: str, worktree: str, **kwargs):
        agents = json.loads((tmp_path / "agents.json").read_text())
        assert not agents[task_id]["alive"]
        return remove_owned(task_id, worktree, **kwargs)

    host.skills.remove_owned = after_agent_stop
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.ledger.records("task")), ticks=24)
    [task] = host.ledger.records("task")
    assert task["outcome"] == "landed"
    worker = host.workers.get(task["id"])
    assert worker["cleanup"]["removed"] is True
    assert not Path(worker["worktree"]).exists()
    tree = git(project, "ls-tree", "-r", "--name-only", "HEAD")
    assert ".agents/skills/docs" not in tree and ".claude/skills/docs" not in tree


def test_changed_or_staged_host_skill_cannot_launder_into_landing(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    host.submit("Update docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    [worker] = host.workers.all()
    installed = Path(worker["skills"][0]["path"])
    original = installed.read_bytes()
    installed.write_text("changed by worker\n")
    assert "installed Skill changed" in host.fleet.v_landable(worker["id"])["reason"]
    installed.write_bytes(original)
    git(Path(worker["worktree"]), "add", "-A")
    assert "worker commit contains host Skill files" in host.fleet.v_landable(worker["id"])["reason"]


def test_changed_parent_link_blocks_cleanup_before_external_file_access(make_host, tmp_path: Path) -> None:
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": approved_skills(tmp_path, ["docs"])})
    host.submit("Update docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    [worker] = host.workers.all()
    root = Path(worker["worktree"])
    installed = root / ".agents" / "skills" / "docs" / "SKILL.md"
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "skills").mkdir()
    (outside / "skills" / "docs").mkdir()
    (outside / "skills" / "docs" / "SKILL.md").write_bytes(installed.read_bytes())
    (root / ".agents").rename(root / ".agents-old")
    os.symlink(outside, root / ".agents")
    assert "skill destination collision" in host.fleet.v_landable(worker["id"])["reason"]
    assert (outside / "skills" / "docs" / "SKILL.md").is_file()


def test_retry_reconciles_files_and_replay_performs_no_effects(make_host, tmp_path: Path, jevscript_bin: str) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    host.submit("Update docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    [worker] = host.workers.all()
    installed = Path(worker["skills"][0]["path"])
    before = installed.stat().st_mtime_ns
    receipt = host.home.task_dir(worker["id"]) / "skill-selection.json"
    host.fleet.begin("retry-dispatch")
    again = host.fleet.call("ensure_skills", {"positional": [worker["id"], worker["worktree"], worker["harness"], ["docs"], "selected", []]})
    assert again["key"] == worker["skill_receipt"] and installed.stat().st_mtime_ns == before
    assert receipt.is_file()
    duplicate = host.fleet.call("ensure_skills", {"positional": [worker["id"], worker["worktree"], worker["harness"], ["docs", "docs"], "selected", []]})
    assert duplicate["key"] == again["key"] and installed.stat().st_mtime_ns == before
    host.workers.update("new-task", worktree=worker["worktree"])
    invalid = host.fleet.call("ensure_skills", {"positional": ["new-task", worker["worktree"], worker["harness"], ["unknown"], "selected", []]})
    assert "unknown selected skill" in invalid["blocked"]
    recording = worker["dispatch_recording"]
    agent_state = tmp_path / "agents.json"
    agent_bytes = agent_state.read_bytes()
    host.close()  # replay must work after the live provider and adapter close
    replay = subprocess.run([jevscript_bin, "replay", recording], capture_output=True, text=True, check=False)
    assert replay.returncode == 0, replay.stderr
    assert installed.stat().st_mtime_ns == before
    assert agent_state.read_bytes() == agent_bytes


def test_installed_cos_wheel_selects_and_hands_off_through_cli(tmp_path: Path, project: Path, jevscript_bin: str) -> None:
    """§9.6/11.6: an offline installed wheel loads its packaged .jev policy,
    selects a pinned Skill, and spawns only after the installed CLI wrote it."""
    root = Path(__file__).resolve().parents[3]
    wheel_dir = tmp_path / "wheels"
    wheel_dir.mkdir()
    subprocess.run(["uv", "build", "--offline", "--wheel", str(root / "examples" / "chief-of-staff"), "--out-dir", str(wheel_dir)], check=True, capture_output=True, text=True)
    wheel = next(wheel_dir.glob("chief_of_staff-*.whl"))
    venv = tmp_path / "venv"
    subprocess.run(["uv", "venv", "--offline", str(venv)], check=True, capture_output=True, text=True)
    python = venv / "bin" / "python"
    subprocess.run(["uv", "pip", "install", "--offline", "--no-deps", "--python", str(python), str(wheel)], check=True, capture_output=True, text=True)
    command = venv / "bin" / "cos"
    home = tmp_path / "installed-home"
    catalog = approved_skills(tmp_path, ["docs"])
    add_support(catalog[0], "references/checklist.md", b"# Required checklist\n")
    rule_file = tmp_path / "rules.json"
    rule_file.write_text(json.dumps(fits("docs") + [
        {"match": {"id": "^work$"}, "answer": {"choice": "ship"}},
        {"match": {"id": "^project$"}, "answer": {"choice": "i0"}},
        {"match": {"id": "^home$"}, "answer": {"choice": "i0"}},
    ]))
    env = {**os.environ, "JEVSCRIPT_BIN": jevscript_bin, "COS_JEV": "fake", "PYTHONPATH": str(root / "sdk" / "python" / "src")}

    def cos(*args: str) -> None:
        run = subprocess.run([str(command), "--home", str(home), *args], env=env, capture_output=True, text=True, check=False)
        assert run.returncode == 0, run.stderr

    cos("init")
    cos("config", "set", "skill_catalog", json.dumps(catalog))
    cos("config", "set", "jev.fake_rules", str(rule_file))
    cos("config", "set", "adapters.crew", json.dumps({"command": [str(python), "-m", "cos.fake_agent"], "env": {"COS_FAKE_AGENT_STATE": str(tmp_path / "agents.json")}}))
    cos("project", "add", "proj", str(project), "--mode", "local-only")
    cos("say", "Update the docs", "--project", "proj")
    cos("tick")
    workers = [json.loads(path.read_text()) for path in (home / "state" / "workers").glob("*.json")]
    assert len(workers) == 1 and workers[0].get("handle")
    [selection] = workers[0]["skills"]
    assert selection["id"] == "docs"
    assert hashlib.sha256(Path(selection["path"]).read_bytes()).hexdigest() == selection["sha256"]
    assert selection["path"] in Path(workers[0]["brief"]).read_text()
    assert (Path(selection["path"]).parent / "references/checklist.md").read_bytes() == b"# Required checklist\n"
    assert selection["tree_sha256"] in Path(workers[0]["brief"]).read_text()


def test_duplicate_approval_or_unpinned_source_never_reaches_spawn(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    write_json(host.home.config_path, {"skill_catalog": catalog + catalog})
    host.submit("Update docs")
    host.tick()
    assert any("duplicate skill id" in decision["question"] for decision in host.decisions.open())
    assert not host.workers.all(live_only=False)
    host.home.set_config("skill_catalog", catalog)
    Path(catalog[0]["path"]).write_text("---\nname: docs\ndescription: changed\n---\n")
    [decision] = host.decisions.open()
    host.decisions.answer(decision["key"], "retry")
    host.tick()
    assert any("pinned source digest changed" in decision["question"] for decision in host.decisions.open())
    assert not host.workers.all(live_only=False)


def test_bad_catalog_parks_dispatch_without_starving_worker(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog},
                     scripts=[{"match": "docs", "steps": ["work", "work", "work", "commit", "done"]}])
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    worker_id = host.workers.all()[0]["id"]
    Path(catalog[0]["path"]).write_text("---\nname: docs\ndescription: edited\n---\n")
    host.submit("Explain something unrelated")
    host.watch(interval=0, max_ticks=8)
    assert any("pinned source digest changed" in d["question"] for d in host.decisions.open())
    assert any(w.get("paused") for w in host.wakes.pending() if w["kind"] == "dispatch")
    assert host.workers.get(worker_id)["history"]


def test_recovery_keeps_first_verified_selection(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs", "tests"])
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    host.submit("Update the docs and tests")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    worker_id = host.workers.all()[0]["id"]
    first = host.workers.get(worker_id)
    host.workers.update(worker_id, handle=None)
    host.backlog.update(worker_id, status="in_flight")
    host.close()
    restarted = make_host(rules=fits("docs", "tests"), yolo=False, config={"skill_catalog": catalog},
                          home_dir=tmp_path / "home", register=False)
    restarted.recover()
    restarted.wakes.push("dispatch", "backlog", ["recovered"])
    run_until(restarted, lambda: bool(restarted.workers.get(worker_id).get("handle")))
    recovered = restarted.workers.get(worker_id)
    assert [skill["id"] for skill in recovered["skills"]] == ["docs"]
    assert recovered["skill_receipt"] == first["skill_receipt"]
    assert not restarted.decisions.open()


def test_large_ready_queue_dispatches_without_orphaning(make_host, tmp_path: Path) -> None:
    host = make_host(rules=fits(), yolo=False, config={"skill_catalog": approved_skills(tmp_path, ["docs"])})
    for index in range(21):
        host.submit(f"Fix typo number {index} in the README")
    run_until(host, lambda: len([w for w in host.workers.all(live_only=False) if w.get("handle")]) == 21, ticks=5)
    assert len(host.workers.all(live_only=False)) == 21
    assert not [d for d in host.decisions.open() if "budget" in d["question"]]


def test_multifile_skill_pinned_installed_and_reconciled(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    add_support(catalog[0], "references/checklist.md", b"# Read this\n")
    add_support(catalog[0], "scripts/check.sh", b"#!/bin/sh\nexit 0\n", mode=0o755)
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    host.submit("Update docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("brief")))
    [worker] = host.workers.all()
    root = Path(worker["skills"][0]["path"]).parent
    assert (root / "references/checklist.md").read_bytes() == b"# Read this\n"
    assert (root / "scripts/check.sh").stat().st_mode & 0o111
    assert worker["skills"][0]["tree_sha256"] in Path(worker["brief"]).read_text()
    assert len(host.skills.owned_paths(worker["id"], worker["worktree"])) == 4
    (root / "references/checklist.md").write_text("changed\n")
    assert "installed Skill changed" in host.fleet.v_landable(worker["id"])["reason"]


def test_multifile_manifest_and_folded_description_at_host_boundary(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    source = Path(catalog[0]["path"])
    source.write_text("---\nname: docs\ndescription: >-\n  Use the docs guide for\n  documentation work.\n---\n# Guide\n")
    catalog[0]["sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
    add_support(catalog[0], "references/guide.md", b"# Guide\n")
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": catalog})
    assert host.skills.catalog()[0]["description"] == "Use the docs guide for documentation work."
    (source.parent / "references/guide.md").write_text("changed\n")
    host.submit("Update docs")
    host.tick()
    assert any("pinned source digest changed" in d["question"] for d in host.decisions.open())
    assert not host.workers.all(live_only=False)


@pytest.mark.parametrize("selected", [False, True])
def test_shared_claude_skills_symlink(make_host, tmp_path: Path, project: Path, selected: bool) -> None:
    (project / ".agents" / "skills").mkdir(parents=True)
    (project / ".agents" / "skills" / ".keep").write_text("")
    (project / ".claude").mkdir()
    os.symlink("../.agents/skills", project / ".claude" / "skills")
    git(project, "add", ".")
    git(project, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "shared skill location")
    host = make_host(rules=fits("docs") if selected else fits(), yolo=False,
                     config={"skill_catalog": approved_skills(tmp_path, ["docs"])})
    host.submit("Update docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    assert bool(worker["skills"]) is selected
    if selected:
        assert Path(worker["skills"][0]["path"]).is_file()
    assert not host.decisions.open()


def test_codex_harness_receives_project_skill_without_claude_link(make_host, tmp_path: Path) -> None:
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": approved_skills(tmp_path, ["docs"])})
    [profile] = host.registry.profiles()
    host.registry.set_profiles([{**profile, "harness": "codex"}])
    host.submit("Update docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    assert worker["harness"] == "codex"
    assert Path(worker["skills"][0]["path"]).is_file()
    assert not (Path(worker["worktree"]) / ".claude" / "skills" / "docs").exists()


@pytest.mark.parametrize("blocked_first", [False, True])
def test_colliding_item_is_held_while_clean_item_starts(make_host, tmp_path: Path, blocked_first: bool) -> None:
    other = tmp_path / "other"
    other.mkdir()
    git(other, "init", "-q", "-b", "main")
    collision = other / ".agents" / "skills" / "docs" / "SKILL.md"
    collision.parent.mkdir(parents=True)
    collision.write_text("project owns this Skill\n")
    git(other, "add", ".")
    git(other, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "own skill")
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": approved_skills(tmp_path, ["docs"])})
    host.registry.add_project("other", str(other), "another project", mode="local-only")
    requests = [("Update the docs B", "other"), ("Update the docs A", "proj")] if blocked_first else [("Update the docs A", "proj"), ("Update the docs B", "other")]
    for text, project_name in requests:
        host.submit(text, project=project_name)
    for wake in list(host.wakes.pending()):
        if wake["kind"] == "request":
            host.handle(wake)
    host.tick()
    items = host.backlog.items()
    clean = next(item for item in items if item["project"] == "proj")
    blocked = next(item for item in items if item["project"] == "other")
    assert host.workers.get(clean["id"])["handle"]
    assert clean["status"] == "in_flight"
    assert blocked["status"] == "queued" and blocked["hold"]["reason"] == f"{blocked['id']}:skills"
    assert any(d["key"] == f"{blocked['id']}:skills" for d in host.decisions.open())
    assert collision.read_text() == "project owns this Skill\n"
    host.tick()
    assert host.workers.get(clean["id"])["handle"]
    [decision] = [d for d in host.decisions.open() if d["key"] == f"{blocked['id']}:skills"]
    host.decisions.answer(decision["key"], "retry")
    host.tick()
    assert host.workers.get(clean["id"])["handle"]
    assert host.backlog.get(blocked["id"])["hold"]
    agents = json.loads((tmp_path / "agents.json").read_text())
    assert agents[clean["id"]]["launches"] == 1
    [decision] = [d for d in host.decisions.open() if d["key"] == f"{blocked['id']}:skills"]
    host.decisions.answer(decision["key"], "dismiss")
    host.tick()
    assert host.backlog.get(blocked["id"])["status"] == "cancelled"
    assert host.workers.get(blocked["id"])["retired"]
    assert not Path(host.workers.get(blocked["id"])["worktree"]).exists()
    assert blocked["id"] not in " ".join(host.recover())


def test_pr_skill_commit_prompts_correction_and_merged_state_stays_visible(make_host, tmp_path: Path, monkeypatch) -> None:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    forge_state = tmp_path / "forge.json"
    forge_state.write_text(json.dumps({"state": "OPEN", "isDraft": False, "statusCheckRollup": [], "mergeable": "MERGEABLE"}))
    gh = fake_bin / "gh"
    merge_calls = tmp_path / "merge-calls"
    gh.write_text(f"#!/bin/sh\nif [ \"$2\" = merge ]; then echo merge >> '{merge_calls}'; fi\ncat '{forge_state}'\n")
    gh.chmod(0o755)
    monkeypatch.setenv("PATH", f"{fake_bin}:{os.environ['PATH']}")
    host = make_host(rules=fits("docs"), yolo=True, mode="direct-PR",
                     config={"skill_catalog": approved_skills(tmp_path, ["docs"])},
                     scripts=[{"match": "docs", "steps": ["work", "work", "commit", "done:https://github.com/example/repo/pull/7"]}])
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    wt = Path(worker["worktree"])
    git(wt, "add", "-A")
    git(wt, "-c", "user.name=w", "-c", "user.email=w@example.invalid", "commit", "-qm", "staged all")
    pushed_head = git(wt, "rev-parse", "HEAD")
    forge_state.write_text(json.dumps({"state": "OPEN", "isDraft": False, "statusCheckRollup": [], "mergeable": "MERGEABLE", "headRefOid": pushed_head}))
    run_until(host, lambda: any("skill_correction" in h["events"] for h in host.workers.get(worker["id"]).get("history", [])), ticks=16)
    state = host.fleet.v_pr_state(worker["id"])
    assert state["state"] == "OPEN" and not state["green"] and not state["skills_ok"]
    assert "host Skill files" in state["reason"]
    assert not host.fleet.v_merge(worker["id"])["merged"]
    assert not merge_calls.exists()
    assert any("Host-owned Skill files" in message.read_text() for message in host.home.inbox_dir(worker["id"]).glob("handled/*.msg"))
    git(wt, "rm", "--cached", "-r", ".agents/skills/docs", ".claude/skills/docs")
    assert not host.fleet.v_pr_state(worker["id"])["skills_ok"]  # index-only correction
    git(wt, "-c", "user.name=w", "-c", "user.email=w@example.invalid", "commit", "--amend", "--no-edit", "-q")
    assert not host.fleet.v_pr_state(worker["id"])["skills_ok"]  # local commit not pushed
    with host.home.status_path(worker["id"]).open("a") as status_file:
        status_file.write(f"done [at={int(time.time())}]: PR https://github.com/example/repo/pull/7\n")
    run_until(host, lambda: sum("skill_correction" in h["events"] for h in host.workers.get(worker["id"]).get("history", [])) >= 2, ticks=12)
    assert not merge_calls.exists()
    assert not host.ledger.records("task")
    forge_state.write_text(json.dumps({"state": "OPEN", "isDraft": False, "statusCheckRollup": [], "mergeable": "MERGEABLE", "headRefOid": "f" * 40}))
    assert "not in the isolated copy" in host.fleet.v_pr_state(worker["id"])["reason"]
    corrected_head = git(wt, "rev-parse", "HEAD")
    forge_state.write_text(json.dumps({"state": "OPEN", "isDraft": False, "statusCheckRollup": [], "mergeable": "MERGEABLE", "headRefOid": corrected_head}))
    assert host.fleet.v_pr_state(worker["id"])["skills_ok"]
    forge_state.write_text(json.dumps({"state": "MERGED", "isDraft": False, "statusCheckRollup": [], "mergeable": "MERGEABLE", "headRefOid": corrected_head}))
    with host.home.status_path(worker["id"]).open("a") as status_file:
        status_file.write(f"done [at={int(time.time())}]: PR https://github.com/example/repo/pull/7\n")
    run_until(host, lambda: bool(host.ledger.records("task")), ticks=16)
    assert host.ledger.records("task")[0]["outcome"] == "landed"
    assert not wt.exists()


def test_merged_pr_with_committed_host_skill_is_reported_and_cleaned(make_host, tmp_path: Path, monkeypatch) -> None:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    gh = fake_bin / "gh"
    forge_state = tmp_path / "forge.json"
    gh.write_text(f"#!/bin/sh\ncat '{forge_state}'\n")
    gh.chmod(0o755)
    monkeypatch.setenv("PATH", f"{fake_bin}:{os.environ['PATH']}")
    host = make_host(rules=fits("docs"), yolo=True, mode="direct-PR",
                     config={"skill_catalog": approved_skills(tmp_path, ["docs"])},
                     scripts=[{"match": "docs", "steps": ["work", "work", "commit", "done:https://github.com/example/repo/pull/7"]}])
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    wt = Path(worker["worktree"])
    git(wt, "add", "-A")
    git(wt, "-c", "user.name=w", "-c", "user.email=w@example.invalid", "commit", "-qm", "staged all")
    forge_state.write_text(json.dumps({"state": "MERGED", "isDraft": False, "statusCheckRollup": [], "mergeable": "MERGEABLE", "headRefOid": git(wt, "rev-parse", "HEAD")}))
    git(wt, "rm", "--cached", "-r", ".agents/skills/docs", ".claude/skills/docs")
    git(wt, "-c", "user.name=w", "-c", "user.email=w@example.invalid", "commit", "-qm", "local removal not pushed")
    run_until(host, lambda: bool(host.workers.get(worker["id"]).get("pr")), ticks=12)
    assert host.fleet.v_pr_state(worker["id"])["state"] == "MERGED"
    assert host.fleet.v_landed(worker["id"])["ok"]
    run_until(host, lambda: bool(host.ledger.records("task")), ticks=12)
    assert host.ledger.records("task")[0]["outcome"] == "landed"
    assert not wt.exists()
    assert any("includes host Skill files" in row["message"] for row in host.decisions.outbox())


def test_deleted_host_copy_and_quoted_support_file_still_land(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    add_support(catalog[0], "references/style guide.md", b"# Style\n")
    host = make_host(rules=fits("docs"), yolo=True, config={"skill_catalog": catalog})
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    shutil.rmtree(Path(worker["worktree"]) / ".agents")
    shutil.rmtree(Path(worker["worktree"]) / ".claude")
    run_until(host, lambda: bool(host.ledger.records("task")), ticks=20)
    assert host.ledger.records("task")[0]["outcome"] == "landed"


def test_quoted_support_file_remains_installed_through_landing(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    add_support(catalog[0], "references/style guide.md", b"# Style\n")
    add_support(catalog[0], "references/café.md", b"# Cafe\n")
    host = make_host(rules=fits("docs"), yolo=True, config={"skill_catalog": catalog})
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    [worker] = host.workers.all()
    assert (Path(worker["skills"][0]["path"]).parent / "references/style guide.md").is_file()
    assert (Path(worker["skills"][0]["path"]).parent / "references/café.md").is_file()
    run_until(host, lambda: bool(host.ledger.records("task")), ticks=20)
    assert host.ledger.records("task")[0]["outcome"] == "landed"


def test_unreadable_support_file_parks_dispatch_and_worker_keeps_progress(make_host, tmp_path: Path) -> None:
    catalog = approved_skills(tmp_path, ["docs"])
    add_support(catalog[0], "references/a.md", b"# A\n")
    host = make_host(rules=fits("docs"), yolo=True, config={"skill_catalog": catalog},
                     scripts=[{"match": "docs", "steps": ["work", "work", "work", "commit", "done"]}])
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.workers.all()) and bool(host.workers.all()[0].get("handle")))
    worker_id = host.workers.all()[0]["id"]
    support = Path(catalog[0]["path"]).parent / "references/a.md"
    support.chmod(0)
    try:
        host.submit("Explain something unrelated")
        host.watch(interval=0, max_ticks=8)
        assert any("cannot read source" in decision["question"] for decision in host.decisions.open())
        assert host.workers.get(worker_id)["history"]
    finally:
        support.chmod(0o644)


def test_missing_worker_copy_uses_normal_landable_recovery(make_host, tmp_path: Path) -> None:
    host = make_host(rules=fits("docs"), yolo=False, config={"skill_catalog": approved_skills(tmp_path, ["docs"])})
    host.submit("Update the docs")
    run_until(host, lambda: any("Land it" in decision["question"] for decision in host.decisions.open()), ticks=16)
    [worker] = host.workers.all()
    host.workers.update(worker["id"], worktree=str(tmp_path / "vanished"))
    assert host.fleet.v_landable(worker["id"])["reason"] == "the isolated copy is gone"
    [decision] = [row for row in host.decisions.open() if "Land it" in row["question"]]
    host.decisions.answer(decision["key"], "land")
    host.tick()
    assert not [row for row in host.decisions.open() if "ls-files" in row["question"]]
    assert any("rebase" in history["events"] for history in host.workers.get(worker["id"])["history"])


def test_cleanup_abort_is_recorded_and_owner_is_told(make_host, tmp_path: Path, monkeypatch) -> None:
    host = make_host(rules=fits("docs"), yolo=True, config={"skill_catalog": approved_skills(tmp_path, ["docs"])})

    def fail_cleanup(*_args, **_kwargs):
        raise SkillError("verified Skill cleanup interrupted")

    monkeypatch.setattr(host.skills, "remove_owned", fail_cleanup)
    host.submit("Update the docs")
    run_until(host, lambda: bool(host.ledger.records("task")), ticks=20)
    [worker] = host.workers.all(live_only=False)
    assert host.ledger.records("task")[0]["outcome"] == "landed"
    assert worker["cleanup"] == {"removed": False, "reason": "verified Skill cleanup interrupted"}
    assert Path(worker["worktree"]).exists()
    assert any("isolated copy was kept: verified Skill cleanup interrupted" in row["message"] for row in host.decisions.outbox())
