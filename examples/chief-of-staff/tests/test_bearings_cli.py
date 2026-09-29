"""Real CoS CLI reads over disposable, fake-backed homes and recordings."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from conftest import BASE_RULES, SRC, run_until

from cos.state.home import iso, now, read_jsonl


def cli(home: Path, jevscript_bin: str, *args: str) -> str:
    env = {**os.environ, "JEVSCRIPT_BIN": jevscript_bin, "COS_JEV": "fake", "PYTHONPATH": str(SRC)}
    result = subprocess.run([sys.executable, "-m", "cos", "--home", str(home), *args], env=env, capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def counts(home: Path) -> tuple[int, int]:
    return len(list((home / "state" / "recordings").glob("*.jsonl"))), sum(row["type"] == "episode" for row in read_jsonl(home / "data" / "ledger.jsonl"))


def test_empty_headline_and_repeated_bearings_are_read_only(make_host, jevscript_bin: str) -> None:
    host = make_host()
    home = host.home.root
    expected = "All quiet: nothing under way and nothing waiting on you."
    assert cli(home, jevscript_bin, "status") == expected
    before = counts(home)
    for _ in range(3):
        assert cli(home, jevscript_bin, "bearings").splitlines()[0] == f"Bearings: {expected}"
    assert counts(home) == before
    assert not list((home / "state" / "recordings").glob("*-bearings-*.jsonl"))


def test_active_fleet_digest_reports_stored_holds_and_dependencies(make_host, jevscript_bin: str) -> None:
    host = make_host(scripts=[{"match": ".", "steps": ["work", "stall"]}])
    host.submit("Update the guide")
    run_until(host, lambda: bool(host.workers.all()))
    assert len(host.workers.all()) == 1
    backlog = host.backlog
    backlog.keep_done = 0
    for item_id, title, status in (("t1", "Finished", "done"), ("t2", "Waiting", "queued")):
        backlog.add({"id": item_id, "title": title, "text": title, "project": "proj"})
        if status == "done":
            backlog.finish(item_id, "done", "landed")
    assert not any(item["id"] == "t1" for item in backlog.items())
    assert backlog.get("t1")["status"] == "done"
    assert any(item["id"] == "t1" for item in read_jsonl(backlog.history_path))
    backlog.add({"id": "t3", "title": "Plain", "text": "Plain", "project": "proj"})
    backlog.add({"id": "t4", "title": "Past", "text": "Past", "project": "proj"})
    backlog.hold("t4", "review", until=now() - 3600)
    backlog.add({"id": "t5", "title": "Future", "text": "Future", "project": "proj"})
    future = now() + 3600
    backlog.hold("t5", "window", until=future)
    backlog.add({"id": "t6", "title": "Held", "text": "Held", "project": "proj", "deps": ["t1", "t2", "missing"]})
    backlog.hold("t6", "owner")
    backlog.add({"id": "t7", "title": "Dependent", "text": "Dependent", "project": "proj", "deps": ["t1", "t2", "missing"]})
    host.decisions.record("choice", "Choose a path?", ["A", "B"])
    home = host.home.root
    before = counts(home)
    status = cli(home, jevscript_bin, "status")
    bearings = cli(home, jevscript_bin, "bearings")
    assert bearings.splitlines()[0] == f"Bearings: {status}"
    assert status.startswith("1 under way, ") and status.endswith("1 waiting on you.")
    assert f"Past [t4]: hold expired {iso(backlog.get('t4')['hold']['until'])}, review" in bearings
    assert f"Future [t5]: held until {iso(future)}, window" in bearings
    assert "Held [t6]: held, owner; after t1 (done), t2 (queued), missing (unknown)" in bearings
    assert "Dependent [t7]: after t1 (done), t2 (queued), missing (unknown)" in bearings
    queued_section = bearings.split("\nQueued and held\n", 1)[1].split("\nRecently finished\n", 1)[0]
    assert "Finished [t1]" not in queued_section
    assert "Finished [t1]: landed" in bearings
    for _ in range(3):
        cli(home, jevscript_bin, "bearings")
    assert counts(home) == before
    assert not list((home / "state" / "recordings").glob("*-bearings-*.jsonl"))


def test_status_route_uses_cli_headline_and_normal_runs_replay(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    rules = [{"match": {"id": "^work$", "text": "going on"}, "answer": {"choice": "status"}}, *BASE_RULES]
    host = make_host(rules=rules)
    rule_file = tmp_path / "rules.json"
    rule_file.write_text(json.dumps(rules))
    host.home.set_config("jev.fake_rules", str(rule_file))
    home = host.home.root
    expected = cli(home, jevscript_bin, "status")
    cli(home, jevscript_bin, "say", "What's going on?")
    cli(home, jevscript_bin, "tick")
    messages = read_jsonl(home / "state" / "outbox.jsonl")
    assert messages[-1]["message"] == expected
    recordings = list((home / "state" / "recordings").glob("*.jsonl"))
    assert any("-on_wake-" in path.name for path in recordings)
    assert any(row["type"] == "episode" and row["subject"] != "backlog" for row in read_jsonl(home / "data" / "ledger.jsonl"))
    assert not any("-bearings-" in path.name for path in recordings)

    cli(home, jevscript_bin, "say", "Update the guide", "--project", "proj")
    cli(home, jevscript_bin, "tick")
    recordings = list((home / "state" / "recordings").glob("*.jsonl"))
    assert any("-on_wake-" in path.name for path in recordings)
    assert any(row["type"] == "episode" and row["subject"] == "backlog" for row in read_jsonl(home / "data" / "ledger.jsonl"))
    for path in recordings:
        replay = subprocess.run([jevscript_bin, "replay", str(path)], capture_output=True, text=True, check=False)
        assert replay.returncode == 0, f"{path.name}: {replay.stderr}"
