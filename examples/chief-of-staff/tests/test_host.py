"""The Chief of Staff end to end, offline: a real runtime, real recordings, a
real git project, the offline Jev and the fake agent adapter process.

Each test names the Chief of Staff behavior it checks.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from conftest import git, run_until

from cos.commands import Commands
from cos.delivery import Delivery
from cos.learning import replay
from cos.state.home import read_jsonl


def only_worker(host) -> dict[str, Any]:
    workers = host.workers.all(live_only=False)
    assert len(workers) == 1, workers
    return host.workers.get(workers[0]["id"])


def finished(host) -> list[dict[str, Any]]:
    return host.ledger.records("task")


def events_of(host, task_id: str) -> list[str]:
    return [e for h in host.workers.get(task_id).get("history", []) for e in h["events"]]


def open_keys(host) -> list[str]:
    return [d["key"] for d in host.decisions.open()]


def test_local_only_work_lands_only_after_the_owner_says_yes(make_host, project: Path) -> None:
    """Delivery + merge authority: without yolo the episode asks and parks;
    the answer resumes it; `done` is reachable only through the guarded
    `landed` event, so the result is verified."""
    host = make_host(yolo=False)
    host.submit("Fix the README title")
    run_until(host, lambda: bool(host.decisions.open()))
    worker = only_worker(host)
    [decision] = host.decisions.open()
    assert decision["key"] == f"{worker['id']}:ask" and "Land it?" in decision["question"]
    assert worker["phase"] == "approved"
    assert git(project, "log", "--oneline").count("\n") == 0, "nothing landed before the answer"
    assert [w["paused"] for w in host.wakes.pending()] == [decision["key"]], "the wake stays queued while parked"

    host.decisions.answer(decision["key"], "yes")
    run_until(host, lambda: bool(finished(host)))
    [task] = finished(host)
    assert task["outcome"] == "landed" and task["verified"] is True
    assert "fake: Fix the README title" in git(project, "log", "--oneline")
    assert not Path(worker["worktree"]).exists(), "cleanup removed the landed copy"
    assert f"cos/{worker['id']}" not in git(project, "branch"), "and its merged branch"
    assert not host.wakes.pending()
    logs = host.ledger.records("log")
    assert any(line["message"] == "wake started" for line in logs)
    assert any(line["message"] == "wake completed" for line in logs)
    assert all(Path(line["recording"]).exists() for line in logs)


def test_a_declined_landing_is_shelved_and_its_work_kept(make_host, project: Path) -> None:
    host = make_host(yolo=False)
    host.submit("Rewrite the intro")
    run_until(host, lambda: bool(host.decisions.open()))
    worker = only_worker(host)
    host.decisions.answer(open_keys(host)[0], "no")
    run_until(host, lambda: bool(finished(host)))
    assert finished(host)[0]["outcome"] == "shelved"
    assert Path(worker["worktree"]).exists(), "declined work is never cleaned up"


def test_a_stalled_worker_is_nudged_through_its_inbox(make_host) -> None:
    """Supervision: a quiet screen is a wake; the machine goes to `stuck`,
    the nudge goes through the durable inbox, the worker acknowledges it."""
    host = make_host(scripts=[{"match": ".", "steps": ["work", "stall", "commit", "done"]}])
    host.submit("Tidy the docs")
    run_until(host, lambda: bool(host.workers.all()) and host.workers.all()[0].get("status_kind") == "working")
    host.tick()  # the worker reaches its stall and the screen stops changing
    worker = only_worker(host)
    host.workers.update(worker["id"], screen_changed_at=worker["screen_changed_at"] - 3600)
    run_until(host, lambda: "nudge" in events_of(host, worker["id"]))
    assert "stalled" in events_of(host, worker["id"])
    inbox = host.home.inbox_dir(worker["id"])
    assert list((inbox / "handled").glob("*.msg")), "the worker acknowledged the nudge by moving it"
    run_until(host, lambda: bool(finished(host)))
    assert finished(host)[0]["nudges"] == 1


def test_a_dead_worker_is_relaunched_in_its_own_copy(make_host) -> None:
    """Recovery: a session that exits before delivering is relaunched, and the
    work continues in the same isolated copy."""
    host = make_host(scripts=[{"match": ".", "steps": ["work", "die", "commit", "done"]}])
    host.submit("Update the changelog")
    run_until(host, lambda: bool(finished(host)))
    task = finished(host)[0]
    assert task["outcome"] == "landed" and task["relaunches"] == 1
    assert "died" in events_of(host, task["id"]) and "relaunch" in events_of(host, task["id"])


def test_a_decision_only_the_owner_can_make_is_asked_and_relayed(make_host) -> None:
    """Escalation: Jev judges that the question needs the owner; the episode
    asks and parks; the answer goes back through the inbox."""
    host = make_host(
        rules=[{"match": {"id": "^needs_owner$"}, "answer": {"noul": 0.9}}],
        scripts=[{"match": ".", "steps": ["work", "ask:Use library A or B?", "commit", "done"]}],
    )
    host.submit("Add CSV export")
    run_until(host, lambda: bool(host.decisions.open()))
    [decision] = host.decisions.open()
    assert "Use library A or B?" in decision["question"]
    host.decisions.answer(decision["key"], "Use A")
    run_until(host, lambda: bool(finished(host)))
    handled = list((host.home.inbox_dir(finished(host)[0]["id"]) / "handled").glob("*.msg"))
    assert any("Answer from the owner: Use A" in p.read_text() for p in handled)


def test_an_in_scope_question_is_answered_without_bothering_the_owner(make_host) -> None:
    host = make_host(scripts=[{"match": ".", "steps": ["work", "ask:Should I also fix the typo nearby?", "commit", "done"]}])
    host.submit("Fix the broken link")
    run_until(host, lambda: bool(finished(host)))
    assert not host.decisions.open()
    assert "in_scope" in events_of(host, finished(host)[0]["id"])


def test_lead_review_gates_landing_and_sends_changes_back(make_host, project: Path) -> None:
    """Lead review: work reaches `approved` only through the reviewer's
    sign-off; requested changes send it back to the worker."""
    host = make_host(
        rules=[{"match": {"id": "^verdict$", "text": "changes"}, "answer": {"choice": "changes"}}, {"match": {"id": "^verdict$"}, "answer": {"choice": "approve"}}],
        scripts=[
            {"match": "^review: ", "steps": ["work", "request_changes:add a test", "idle"]},
            {"match": ".", "steps": ["work", "commit", "done", "idle", "done"]},
        ],
        config={"review_profile": "lead"},
    )
    host.registry.set_profiles(host.registry.profiles() + [{"name": "lead", "rule": "reviews other workers' changes", "harness": "codex"}])
    host.submit("Add input validation")
    run_until(host, lambda: bool(host.workers.all()) and "changes_requested" in events_of(host, host.workers.all()[0]["id"]))
    worker = only_worker(host)
    assert "request_review" in events_of(host, worker["id"]) and "signed_off" not in events_of(host, worker["id"])
    assert any("add a test" in p.read_text() for p in (host.home.inbox_dir(worker["id"]) / "handled").glob("*.msg"))
    assert git(project, "log", "--oneline").count("\n") == 0, "nothing lands without a sign-off"


def test_a_scout_writes_a_report_and_changes_no_code(make_host, project: Path) -> None:
    """Two task shapes: a scout's deliverable is a report, not a change."""
    host = make_host(rules=[{"match": {"id": "^work$"}, "answer": {"choice": "scout"}}])
    host.submit("Investigate why the build is slow")
    run_until(host, lambda: bool(finished(host)))
    task = finished(host)[0]
    assert task["kind"] == "scout" and task["outcome"] == "landed"
    assert (host.home.task_dir(task["id"]) / "report.md").exists()
    assert git(project, "log", "--oneline").count("\n") == 0


def test_a_pull_request_is_merged_under_yolo_once_green(make_host) -> None:
    """PR watch and merge authority for direct-PR projects."""
    merged: list[str] = []

    class Forge(Delivery):
        def pr_state(self, url: str) -> dict[str, Any]:
            return {"state": "MERGED" if merged else "OPEN", "green": True}

        def merge(self, url: str) -> dict[str, Any]:
            merged.append(url)
            return {"merged": True}

    host = make_host(mode="direct-PR", yolo=True, scripts=[{"match": ".", "steps": ["work", "commit", "done:https://github.com/o/r/pull/7"]}])
    host.delivery = Forge()
    host.submit("Add a health endpoint")
    run_until(host, lambda: bool(finished(host)))
    assert merged == ["https://github.com/o/r/pull/7"]
    assert "merge" in events_of(host, finished(host)[0]["id"])


def test_cleanup_never_destroys_unlanded_work(project: Path, tmp_path: Path) -> None:
    delivery = Delivery()
    record = {"name": "proj", "path": str(project), "default_branch": "main", "branch_prefix": "cos/"}
    wt = delivery.create_worktree(record, "t9", tmp_path / "wts")
    Path(wt["path"], "new.txt").write_text("unsaved\n")
    assert delivery.cleanup(record, wt["path"], wt["branch"]) == {"removed": False, "reason": "the isolated copy has uncommitted changes"}
    git(Path(wt["path"]), "add", ".")
    git(Path(wt["path"]), "-c", "user.name=t", "-c", "user.email=t@e.invalid", "commit", "-qm", "work")
    result = delivery.cleanup(record, wt["path"], wt["branch"])
    assert result["removed"] is False and "not in main" in result["reason"]
    assert Path(wt["path"], "new.txt").exists()
    assert not delivery.landed(record, wt["branch"])["ok"]
    assert delivery.land(record, wt["path"], wt["branch"])["landed"]
    assert delivery.landed(record, wt["branch"])["ok"]
    assert delivery.cleanup(record, wt["path"], wt["branch"])["removed"]


def test_a_branch_behind_main_is_sent_back_to_rebase(make_host, project: Path) -> None:
    host = make_host(scripts=[{"match": ".", "steps": ["work", "commit", "done", "idle"]}])
    host.submit("Rename the config key")
    run_until(host, lambda: bool(host.workers.all()) and host.workers.all()[0].get("status_kind") == "working")
    (project / "other.txt").write_text("moved on\n")
    git(project, "add", ".")
    git(project, "-c", "user.name=t", "-c", "user.email=t@e.invalid", "commit", "-qm", "main moved")
    run_until(host, lambda: "rebase" in events_of(host, host.workers.all()[0]["id"]))
    assert not finished(host), "nothing lands while the branch is not a clean fast-forward"


def test_risky_requests_wait_for_a_go_ahead(make_host) -> None:
    host = make_host(rules=[{"match": {"id": "^risky$"}, "answer": {"noul": 0.95}}])
    host.submit("Drop the old production table")
    host.tick()
    [decision] = host.decisions.open()
    assert "Start it?" in decision["question"] and not host.workers.all() and not host.backlog.items()
    host.decisions.answer(decision["key"], "yes")
    run_until(host, lambda: bool(finished(host)))


def test_a_question_survives_a_restart_and_resumes_from_its_recording(make_host) -> None:
    """Crash safety: a run parked at a question is rebuilt by replaying its
    recording with no model calls, and the answer continues it live."""
    host = make_host(rules=[{"match": {"id": "^risky$"}, "answer": {"noul": 0.95}}])
    host.submit("Rotate the production keys")
    host.tick()
    [decision] = host.decisions.open()
    host.close()

    second = make_host(rules=[{"match": {"id": "^risky$"}, "answer": {"noul": 0.95}}], register=False)
    calls_before = len(second.session.fake.requests)
    second.decisions.answer(decision["key"], "yes")
    second.tick()
    assert second.backlog.items(), "the resumed episode queued the work"
    resumed = [e for e in second.ledger.records("episode") if e.get("resumed")]
    assert resumed and Path(resumed[0]["recording"] + ".resumed.jsonl").exists()
    assert len(second.session.fake.requests) == calls_before, "replaying up to the question asked Jev nothing"


def test_effects_are_idempotent_when_a_wake_runs_again(make_host) -> None:
    """A crashed wake is run again from the start; its effects happen once."""
    host = make_host()
    host.submit("Fix the footer")
    run_until(host, lambda: bool(host.workers.all()))
    worker = only_worker(host)
    for _ in range(2):  # the same wake, run twice
        host.fleet.begin("wake-42")
        host.fleet.call("steer", {"positional": [worker["id"], "hello"]})
    assert len(list(host.home.inbox_dir(worker["id"]).rglob("*.msg"))) == 1
    host.fleet.begin("wake-43")
    host.fleet.call("steer", {"positional": [worker["id"], "hello"]})
    assert len(list(host.home.inbox_dir(worker["id"]).rglob("*.msg"))) == 2, "a new wake acts again"


def test_restart_is_a_non_event(make_host) -> None:
    """Session start and recovery: durable records, not memory, carry the
    work across a host restart, including a copy whose spawn never finished."""
    host = make_host(scripts=[{"match": ".", "steps": ["work", "work", "commit", "done"]}])
    host.submit("Write the upgrade guide")
    host.tick()
    worker = only_worker(host)
    host.close()

    second = make_host(scripts=[{"match": ".", "steps": ["work", "work", "commit", "done"]}], register=False)
    second.recover()
    run_until(second, lambda: bool(finished(second)))
    assert finished(second)[0]["id"] == worker["id"]

    item = second.backlog.add({"text": "Write the migration notes", "project": "proj"})
    second.fleet.v_worktree(item["id"], "proj")
    second.backlog.start(item["id"])
    assert any("dispatching again" in n for n in second.recover())
    run_until(second, lambda: len(finished(second)) == 2)


def test_away_mode_holds_routine_news_for_the_digest(make_host) -> None:
    host = make_host()
    host.home.set_mode("away")
    host.submit("Fix the footer")
    run_until(host, lambda: bool(finished(host)))
    assert not read_jsonl(host.home.state / "outbox.jsonl"), "nothing interrupted the owner"
    with Commands(host.home, host_factory=lambda _: host) as cos:
        reply = cos.back()
        cos._host = None  # the fixture owns the host
    assert "Landed 'Fix the footer'" in reply.text
    assert host.home.mode == "normal"


def test_status_questions_and_preferences_are_answered_not_queued(make_host) -> None:
    host = make_host(rules=[{"match": {"id": "^work$", "text": "prefer"}, "answer": {"choice": "memory"}}, {"match": {"id": "^work$", "text": "going on"}, "answer": {"choice": "status"}}])
    host.submit("I prefer squash merges")
    host.submit("What's going on?")
    host.tick()
    assert "squash merges" in host.memory.preferences()
    assert not host.backlog.items()
    messages = [r["message"] for r in read_jsonl(host.home.state / "outbox.jsonl")]
    assert any("All quiet" in m or "under way" in m for m in messages)


def test_second_mates_take_work_in_their_scope(make_host) -> None:
    host = make_host(rules=[{"match": {"id": "^home$"}, "answer": {"choice": "i1"}}])
    host.registry.add_mate("docs", "documentation work")
    host.submit("Document the API")
    run_until(host, lambda: bool(read_jsonl(host.home.state / "mates" / "docs.log")))
    [report] = read_jsonl(host.home.state / "mates" / "docs.log")
    assert report["outcome"] == "landed"
    assert not host.backlog.items(), "the parent never tracked the mate's work itself"


def test_every_wake_replays_from_its_recording_with_no_model_calls(make_host) -> None:
    """Everything is recorded: a supervision episode replays standalone."""
    host = make_host()
    host.submit("Fix the README title")
    run_until(host, lambda: bool(finished(host)))
    recording = finished(host)[0]["recordings"][-2]
    before = len(host.session.fake.requests)
    final = replay(recording)
    assert final is not None and final["kind"] == "done"
    assert final["outputs"]["result"]["phase"] == "done" and final["outputs"]["result"]["verified"] is True
    assert len(host.session.fake.requests) == before, "replay made no model call"


def test_learning_turns_repeated_work_into_a_live_playbook(make_host) -> None:
    """Learning loop: record, classify, encode, prove by replay, go live
    without approval, match first at intake; then disable and revert."""
    rules = [
        {"match": {"id": "^kinds\\["}, "answer": {"choice": "docs_change"}},
        {"match": {"id": "^repeated$"}, "answer": {"noul": 0.9}},
        {"match": {"id": "^nature$"}, "answer": {"choice": "automatable"}},
        {"match": {"id": "^efficiency$"}, "answer": {"score": 1}},
        {"match": {"id": "^routine$"}, "answer": {"noul": 0.9}},
        {"match": {"id": "^pb$"}, "answer": {"item": "documentation"}},
        {"match": {"id": "^applies$", "text": "(?i)doc|readme"}, "answer": {"noul": 0.9}},
    ]
    host = make_host(rules=rules, config={"learn_every": 99})
    for text in ("Fix the README typo", "Document the CLI flags", "Correct the docs index"):
        host.submit(text)
        run_until(host, lambda n=len(finished(host)): len(finished(host)) > n)
    result = host.learning.run_pass()
    [adopted] = result["adopted"]
    assert adopted["live"] and adopted["verdict"]["replayed"] == 3
    trail = [e["event"] for e in host.learning.playbooks.audit_trail("pb_docs_change")]
    assert trail[-2:] == ["verified", "live"]

    host.submit("Fix the docs sidebar")
    run_until(host, lambda: len(finished(host)) == 4)
    assert finished(host)[-1]["playbook"] == "pb_docs_change"

    host.learning.playbooks.set_active("pb_docs_change", False)
    host.submit("Fix the docs footer")
    run_until(host, lambda: len(finished(host)) == 5)
    assert finished(host)[-1]["playbook"] is None, "a disabled playbook is not used"
    assert host.learning.playbooks.revert("pb_docs_change")["active"] is False


def test_commands_reply_with_short_outcomes(make_host) -> None:
    host = make_host()
    with Commands(host.home, host_factory=lambda _: host) as cos:
        reply = cos.say("Fix the logo alt text")
        assert "Queued 'Fix the logo alt text'" in reply.text and "Started 1 worker" in reply.text
        assert cos.status().text.startswith("1 under way")
        cos._host = None
