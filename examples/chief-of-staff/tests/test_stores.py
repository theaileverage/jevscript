"""Durable records: backlog, inbox, status log, decisions, registry, ledger,
the adapter bridge and the keychain rule."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest
from conftest import fake_agent_entry

from cos.state.backlog import Backlog
from cos.state.decisions import Decisions
from cos.capabilities.agent import AdapterError, ExternalAdapter, from_config
from cos.state.home import Home, LockHeld
from cos.state.inbox import Inbox, StatusLog, parse_status
from cos.jevbin import SetupError, keychain_key
from cos.state.ledger import words_count, words_minutes
from cos.state.memory import Memory
from cos.state.registry import Registry


def test_backlog_keeps_recent_done_and_archives_the_rest(tmp_path: Path) -> None:
    backlog = Backlog(Home(tmp_path).init(), keep_done=2)
    ids = [backlog.add({"text": f"task {n}"})["id"] for n in range(4)]
    for task_id in ids:
        backlog.finish(task_id, "done", "landed")
    assert [i["id"] for i in backlog.items()] == ids[2:]
    assert [i["id"] for i in backlog.done_history()] == ids, "archived history is still readable"
    assert backlog.get(ids[0])["outcome"] == "landed"


def test_holds_and_release(tmp_path: Path) -> None:
    backlog = Backlog(Home(tmp_path).init())
    item = backlog.add({"text": "risky thing", "hold": {"reason": "owner", "until": None}})
    assert backlog.snapshot()[0]["hold"] == {"reason": "owner", "until": None}
    backlog.release(item["id"])
    assert backlog.snapshot()[0]["hold"] is None


def test_inbox_records_are_sequenced_and_acknowledged_by_moving(tmp_path: Path) -> None:
    home = Home(tmp_path).init()
    inbox = Inbox(home, "t1")
    first = inbox.post("one")
    second = inbox.post("two\nlines")
    assert (first.name, second.name) == ("001.msg", "002.msg")
    assert "two\nlines" in second.read_text()
    first.rename(inbox.dir / "handled" / first.name)
    assert [Path(u["path"]).name for u in inbox.unacked()] == ["002.msg"]
    assert inbox.post("three").name == "003.msg", "numbering continues past handled records"


def test_status_log_reads_only_what_is_new(tmp_path: Path) -> None:
    home = Home(tmp_path).init()
    log = StatusLog(home, "t1")
    log.append("working", "started")
    lines, offset = log.read(0)
    assert [l.kind for l in lines] == ["working"]
    log.append("done", "PR https://github.com/o/r/pull/9.")
    lines, _ = log.read(offset)
    assert lines[0].kind == "done" and lines[0].url == "https://github.com/o/r/pull/9"
    assert parse_status("not a status line") is None
    assert parse_status("needs-decision [at=12]: pick one").at == 12


def test_decisions_are_asked_once_answered_and_consumed_once(tmp_path: Path) -> None:
    decisions = Decisions(Home(tmp_path).init(), echo=False)
    decisions.record("t1:land", "Land it?", ["yes", "no"])
    decisions.record("t1:land", "Land it? (asked again)", ["yes", "no"])
    assert [d["question"] for d in decisions.open()] == ["Land it?"]
    decisions.answer("t1:la", "yes")  # an unambiguous prefix is enough
    assert decisions.answered_for("t1")[0]["answer"] == "yes"
    assert decisions.consume("t1:land")["answer"] == "yes"
    assert decisions.consume("t1:land") is None


def test_registry_projects_mates_and_memory(tmp_path: Path, project: Path) -> None:
    home = Home(tmp_path / "home").init()
    registry = Registry(home)
    record = registry.add_project("proj", str(project), "desc", mode="direct-PR", yolo=True)
    assert record["default_branch"] == "main" and record["mode"] == "direct-PR"
    with pytest.raises(ValueError):
        registry.add_project("x", str(project), "d", mode="sometimes")
    with pytest.raises(RuntimeError):
        registry.remove_project("proj", ["t1"])  # never with work under way
    mate = registry.add_mate("ops", "infrastructure")
    assert Path(mate["home"], "data", "projects.json").exists(), "a mate starts with the parent's projects"
    assert registry.homes()[0]["name"] == "main" and registry.homes()[1]["name"] == "ops"
    memory = Memory(home)
    assert memory.remember_preference("Prefer small PRs") is True
    assert memory.remember_preference("Prefer  small PRs") is False, "whitespace does not make a new fact"
    assert "Prefer small PRs" in memory.preferences()


def test_numbers_become_words_before_jev_sees_them() -> None:
    assert words_minutes(0.5) == "about a minute"
    assert words_minutes(25) == "about 25 minutes"
    assert words_minutes(180) == "about 3 hours"
    assert words_count(1, "nudge") == "one nudge" and words_count(3, "nudge") == "three nudges"


def test_session_lock_is_exclusive_and_stale_locks_are_taken_over(tmp_path: Path) -> None:
    home = Home(tmp_path).init()
    (home.state / "session.lock").write_text("999999 stale\n")  # no such process
    with home.lock():
        child = subprocess.run(
            [sys.executable, "-c", f"import sys; sys.path.insert(0, {str(Path(__file__).parents[1] / 'src')!r});"
             f"from cos.state.home import Home, LockHeld\n"
             f"try:\n  Home({str(tmp_path)!r}).lock().__enter__()\nexcept LockHeld: print('held')"],
            capture_output=True, text=True,
        )
        assert child.stdout.strip() == "held"
    assert not (home.state / "session.lock").exists()
    _ = LockHeld


def test_external_adapter_speaks_the_jsonl_protocol(tmp_path: Path, project: Path) -> None:
    adapter = from_config(fake_agent_entry(tmp_path), "agent", "crew")
    try:
        handle = adapter.call("spawn", {"positional": [], "named": {"prompt": "no brief", "in": {"path": str(project)}, "name": "w1"}})
        assert handle["id"] == "w1"
        observation = adapter.observe(handle)
        assert {"status", "last_message", "tail", "exit_code"} <= set(observation)
        adapter.verb("send", handle, "hello")
        assert "> hello" in adapter.observe(handle)["tail"]
        with pytest.raises(AdapterError):
            adapter.verb("dance", handle)
        again = adapter.call("spawn", {"positional": [], "named": {"prompt": "x", "name": "w1"}})
        assert again["launch"] == handle["launch"], "spawning a live name reattaches instead of duplicating"
    finally:
        adapter.close()


def test_a_hung_adapter_is_stopped_and_reported_retryable(tmp_path: Path) -> None:
    adapter = ExternalAdapter([sys.executable, "-c", "import time; time.sleep(60)"], "agent", "crew", timeout=1)
    with pytest.raises(AdapterError) as caught:
        adapter.verb("observe", {"id": "x"})
    assert caught.value.retryable
    assert adapter._process is None


def test_the_adapter_never_inherits_the_jev_key(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("TYPESAFE_API_KEY", "secret-value")
    script = "import json,os,sys\nfor l in sys.stdin: print(json.dumps({'result': os.environ.get('TYPESAFE_API_KEY')}), flush=True)"
    adapter = ExternalAdapter([sys.executable, "-c", script], "tool", "probe", timeout=10)
    try:
        assert adapter.verb("anything") is None
    finally:
        adapter.close()


def test_keychain_errors_name_the_service_never_a_key() -> None:
    def missing(argv, **_):
        assert argv[:2] == ["security", "find-generic-password"] and "-w" in argv
        return subprocess.CompletedProcess(argv, 44, "", "not found")

    if not os.path.exists("/usr/bin/security"):
        pytest.skip("no macOS keychain here")
    with pytest.raises(SetupError, match="typesafe-test"):
        keychain_key("typesafe-test", run=missing)

    def found(argv, **_):
        assert argv == ["security", "find-generic-password", "-s", "typesafe-test", "-w", "-a", "me"]
        return subprocess.CompletedProcess(argv, 0, "k-123\n", "")

    assert keychain_key("typesafe-test", "me", run=found) == "k-123"
