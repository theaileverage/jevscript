"""Section 6.4a conversation routing with durable, scoped host correlation."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

import pytest

from cos.commands import Commands
from cos.state.home import write_json


def routed(host, message_id: str):
    result = host.tick()
    rows = [r["result"] for r in result["handled"] if r["kind"] == "request" and r["result"] and r["result"]["id"]]
    return next(r for r in rows if r["id"] == host.threads.receipt(host.threads.scope({"source": "cli", "project": "proj", "channel": "general"}), message_id)["request_id"])


def test_new_semantic_continue_none_and_explicit_reply_priority(make_host):
    host = make_host(rules=[{"match": {"id": "^matched$"}, "answer": {"choice": "i0"}}])
    first = host.submit("Improve search performance", project="proj", channel="general", message_id="m1", after=["unreleased"])
    first_result = routed(host, "m1")
    thread_id = first_result["thread_id"]
    task_id = first_result["task_id"]
    assert first_result["route"] == "backlog" and thread_id.startswith("th-") and task_id.startswith("t")
    assert first["id"] != thread_id != task_id

    host.submit("The search index is still slow", project="proj", channel="general", message_id="m2")
    second = routed(host, "m2")
    assert (second["route"], second["thread_id"], second["task_id"]) == ("continue", thread_id, task_id)
    assert "still slow" in host.backlog.get(task_id)["notes"]

    host.session.fake.rules.insert(0, {"match": {"id": "^matched$"}, "answer": {"choice": "none"}})
    host.submit("Unrelated billing request", project="proj", channel="general", message_id="m3")
    third = routed(host, "m3")
    assert third["thread_id"] != thread_id and third["task_id"] != task_id
    matches = sum("matched" in req.get("questions", {}) for req in host.session.fake.requests)
    host.submit("Completely different text", project="proj", channel="general", message_id="m4", reply_to="m1")
    fourth = routed(host, "m4")
    assert fourth["thread_id"] == thread_id, "explicit reply wins over semantic none"
    assert sum("matched" in req.get("questions", {}) for req in host.session.fake.requests) == matches, "explicit relation needs no semantic model call"


def test_inferred_project_receipt_supports_explicit_project_reply(make_host):
    host = make_host()
    host.submit("Update the project guide", channel="general", message_id="inferred", after=["unreleased"])
    first = routed(host, "inferred")
    scope = {"source": "cli", "project": "proj", "channel": "general"}
    receipt = host.threads.receipt(scope, "inferred")
    assert receipt["thread_id"] == first["thread_id"]
    assert host.threads.get(first["thread_id"])["scope"] == scope
    assert host.threads.candidates({"source": "cli", "project": "proj", "channel": "general"})[0]["id"] == first["thread_id"]
    host.submit("Also update its examples", project="proj", channel="general", message_id="explicit", reply_to="inferred")
    second = routed(host, "explicit")
    assert second["route"] == "continue"
    assert second["thread_id"] == first["thread_id"]


def test_inferred_project_routes_no_project_followups(make_host, project):
    host = make_host(rules=[{"match": {"id": "^matched$"}, "answer": {"choice": "i0"}}])
    host.registry.add_project("other", str(project), "another scope")
    host.submit("Improve the search guide", channel="general", message_id="start", after=["unreleased"])
    first = routed(host, "start")
    host.submit("Update a separate guide", project="other", channel="general", message_id="other-start", after=["unreleased"])
    host.tick()
    other = host.threads.receipt({"source": "cli", "project": "other", "channel": "general"}, "other-start")
    host.submit("Also fix the examples", channel="general", message_id="reply", reply_to="start")
    explicit = routed(host, "reply")
    assert explicit["route"] == "continue"
    assert explicit["thread_id"] == first["thread_id"]
    host.submit("The search guide still needs work", channel="general", message_id="semantic")
    semantic = routed(host, "semantic")
    assert semantic["route"] == "continue"
    assert semantic["thread_id"] == first["thread_id"]
    assert semantic["thread_id"] != other["thread_id"]


def test_clarified_project_controls_backlog_and_thread_scope(make_host, project):
    host = make_host(rules=[{"match": {"id": "^ambiguous$"}, "answer": {"noul": 0.95}}])
    host.registry.add_project("other", str(project), "the clarified project")
    host.submit("Update the guide", channel="general", message_id="clarified", after=["unreleased"])
    host.tick()
    [decision] = host.decisions.open()
    host.session.fake.rules.insert(0, {"match": {"id": "^project$"}, "answer": {"choice": "i1"}})
    host.session.fake.rules.insert(0, {"match": {"id": "^ambiguous$"}, "answer": {"noul": 0.15}})
    host.decisions.answer(decision["key"], "Use the other project")
    host.tick()
    scope = {"source": "cli", "project": "other", "channel": "general"}
    receipt = host.threads.receipt(scope, "clarified")
    assert receipt["thread_id"]
    assert host.threads.get(receipt["thread_id"])["scope"] == scope
    assert host.backlog.get(receipt["task_id"])["project"] == "other"



@pytest.mark.parametrize("relation_field", ["reply_to", "native_thread"])
def test_ambiguous_explicit_relation_asks_for_project(make_host, project, relation_field):
    host = make_host()
    host.registry.add_project("other", str(project), "another scope")
    roots = {}
    for name in ("proj", "other"):
        message_id = "shared" if relation_field == "reply_to" else "shared-native"
        native_thread = "shared-native" if relation_field == "native_thread" else None
        host.submit("Update the guide", project=name, channel="general", message_id=message_id, native_thread=native_thread, after=["unreleased"])
        host.tick()
        roots[name] = host.threads.receipt({"source": "cli", "project": name, "channel": "general"}, message_id)
    target = "shared" if relation_field == "reply_to" else "shared-native"
    host.submit("Also update the examples", channel="general", message_id="followup", **{relation_field: target})
    host.tick()
    [decision] = host.decisions.open()
    assert set(decision["options"]) == {"proj", "other"}
    assert len(host.backlog.items()) == 2
    host.decisions.answer(decision["key"], "other")
    host.tick()
    receipt = host.threads.receipt({"source": "cli", "project": "other", "channel": "general"}, "followup")
    assert receipt["route"] == "continue"
    assert receipt["thread_id"] == roots["other"]["thread_id"]
    assert receipt["thread_id"] != roots["proj"]["thread_id"]


def test_inferred_project_rejects_another_requests_message_id(make_host, project):
    host = make_host(rules=[{"match": {"id": "^project$"}, "answer": {"choice": "i1"}}])
    host.registry.add_project("other", str(project), "another scope")
    first = host.submit("Original request", project="other", channel="general", message_id="collision", after=["unreleased"])
    host.tick()
    scope = {"source": "cli", "project": "other", "channel": "general"}
    original = host.threads.receipt(scope, "collision")
    second = host.submit("Different request", channel="general", message_id="collision", after=["unreleased"])
    with pytest.raises(ValueError, match="another request"):
        host.tick()
    assert host.threads.receipt(scope, "collision")["request_id"] == first["id"]
    assert host.threads.receipt({"source": "cli", "project": "", "channel": "general"}, "collision")["request_id"] == second["id"]
    assert host.threads.get(original["thread_id"])["messages"][0]["request_id"] == first["id"]
    assert len(host.backlog.items()) == 1


def test_missing_parent_native_thread_and_scope_separation(make_host):
    host = make_host()
    host.submit("Start conversation", project="proj", channel="general", message_id="native-1", native_thread="native-1", after=["unreleased"])
    first = routed(host, "native-1")
    host.submit("Reply here", project="proj", channel="general", message_id="native-2", native_thread="native-1")
    assert routed(host, "native-2")["thread_id"] == first["thread_id"]
    host.submit("Where did it go?", project="proj", channel="general", message_id="missing", reply_to="not-here")
    missing = routed(host, "missing")
    assert missing["route"] == "ask_thread" and missing["thread_id"] is None
    host.submit("Same words, other channel", project="proj", channel="private", message_id="native-3", reply_to="native-1")
    host.tick()
    other_scope = {"source": "cli", "project": "proj", "channel": "private"}
    assert host.threads.receipt(other_scope, "native-3")["route"] == "ask"
    assert len([t for t in host.threads.all() if t["scope"]["channel"] == "private"]) == 0


def test_project_scope_and_low_confidence_do_not_reuse_a_thread(make_host, project):
    host = make_host(rules=[{"match": {"id": "^matched$"}, "answer": {"choice": "i0", "confidence": 0.4}}])
    host.registry.add_project("other", str(project), "another scope")
    host.submit("Update the deployment guide", project="proj", channel="general", message_id="s1", after=["unreleased"])
    first = routed(host, "s1")
    host.submit("Update the deployment guide", project="other", channel="general", message_id="s2", after=["unreleased"])
    host.tick()
    other = host.threads.receipt({"source": "cli", "project": "other", "channel": "general"}, "s2")
    assert other["thread_id"] != first["thread_id"]
    host.submit("Update the deployment guide", project="proj", channel="general", message_id="s3", after=["unreleased"])
    third = routed(host, "s3")
    assert third["thread_id"] != first["thread_id"], "low confidence starts a new conversation"


def test_retry_restart_and_caller_receives_selected_thread(make_host):
    host = make_host()
    with Commands(host.home, host_factory=lambda _: host) as cos:
        first = cos.say("Update onboarding docs", project="proj", channel="general", message_id="same", after=["unreleased"])
        assert "thread th-" in first.text and "task t" in first.text
        scope = {"source": "cos", "project": "proj", "channel": "general"}
        receipt = host.threads.receipt(scope, "same")
        duplicate = cos.say("Update onboarding docs", project="proj", channel="general", message_id="same")
        assert "Already received" in duplicate.text and duplicate.data["thread_id"] == receipt["thread_id"]
        with pytest.raises(ValueError, match="different message"):
            cos.say("A different request", project="proj", channel="general", message_id="same")
        cos._host = None
    host.close()
    second = make_host(register=False)
    again = second.submit("Update onboarding docs", source="cos", project="proj", channel="general", message_id="same")
    assert again["duplicate"] and again["receipt"]["thread_id"] == receipt["thread_id"]
    assert len(second.threads.all()) == 1
    assert len(second.backlog.items()) == 1
    # Simulate a crash after persisting the choice but before wake ack.
    write_json(second.home.state / "requests" / f"{receipt['request_id']}.json", receipt["request"])
    calls = len(second.session.fake.requests)
    second.tick()
    assert len(second.session.fake.requests) == calls
    assert len(second.backlog.items()) == 1


def test_concurrent_delivery_of_one_message_creates_one_request(make_host):
    host = make_host()
    with ThreadPoolExecutor(max_workers=2) as pool:
        requests = list(pool.map(lambda _: host.submit("Update the API docs", project="proj", channel="general", message_id="race", after=["unreleased"]), range(2)))
    assert requests[0]["id"] == requests[1]["id"]
    assert len(list((host.home.state / "requests").glob("*.json"))) == 1
    routed(host, "race")
    assert len(host.threads.all()) == 1 and len(host.backlog.items()) == 1


def test_semantic_candidates_are_bounded_to_same_scope(make_host):
    host = make_host()
    for index in range(12):
        request = {"id": f"r{index}", "message_id": f"m{index}", "text": f"Subject {index}", "source": "cli", "project": "proj", "channel": "general", "at": "now"}
        host.threads.enqueue(request)
        host.threads.apply(request, {"route": "new"})
    candidates = host.threads.candidates({"source": "cli", "project": "proj", "channel": "general"})
    assert len(candidates) == 8
    assert host.threads.candidates({"source": "cli", "project": "proj", "channel": "private"}) == []


def test_active_worker_receives_followup_in_its_inbox(make_host):
    host = make_host(scripts=[{"match": ".", "steps": ["work", "stall", "commit", "done"]}])
    host.submit("Fix startup logging", project="proj", channel="general", message_id="active-1")
    first = routed(host, "active-1")
    task_id = first["task_id"]
    assert host.backlog.get(task_id)["status"] == "in_flight"
    host.submit("Include the service name", project="proj", channel="general", message_id="active-2", reply_to="active-1")
    second = routed(host, "active-2")
    assert second["task_id"] == task_id and second["thread_id"] == first["thread_id"]
    assert second["worker_id"].startswith("w-") and second["worker_id"] not in (first["id"], task_id, first["thread_id"])
    inbox = host.home.inbox_dir(task_id)
    messages = list(inbox.glob("*.msg")) + list((inbox / "handled").glob("*.msg"))
    assert any("Include the service name" in p.read_text() for p in messages)


def test_completed_task_followup_keeps_thread_and_creates_linked_task(make_host):
    host = make_host(rules=[{"match": {"id": "^matched$"}, "answer": {"choice": "i0"}}])
    host.submit("Fix README heading", project="proj", channel="general", message_id="done-1")
    result = routed(host, "done-1")
    old_task = result["task_id"]
    host.backlog.finish(old_task, "done", "landed")
    host.submit("Also fix the footer", project="proj", channel="general", message_id="done-2", reply_to="done-1", after=["unreleased"])
    followup = routed(host, "done-2")
    assert followup["thread_id"] == result["thread_id"]
    assert followup["task_id"] != old_task
    assert host.backlog.get(followup["task_id"])["thread_id"] == result["thread_id"]


def test_taskless_thread_followup_is_assessed_and_keeps_identity(make_host):
    host = make_host(rules=[{"match": {"id": "^work$"}, "answer": {"choice": "status"}}])
    host.submit("What is in progress?", project="proj", channel="general", message_id="status-1")
    first = routed(host, "status-1")
    assert first["route"] == "status" and first["task_id"] is None
    host.session.fake.rules.insert(0, {"match": {"id": "^work$"}, "answer": {"choice": "ship"}})
    host.submit("Now update the guide", project="proj", channel="general", message_id="status-2", reply_to="status-1", after=["unreleased"])
    second = routed(host, "status-2")
    assert second["route"] == "backlog"
    assert second["thread_id"] == first["thread_id"]
    assert second["task_id"] != first["id"]
    assert host.backlog.get(second["task_id"])["thread_id"] == first["thread_id"]


def test_restart_repairs_thread_link_after_receipt_before_ack(make_host):
    host = make_host()
    host.submit("Update the docs", project="proj", channel="general", message_id="repair-1", after=["unreleased"])
    result = routed(host, "repair-1")
    task_id = result["task_id"]
    host.backlog.update(task_id, thread_id=None)
    scope = {"source": "cli", "project": "proj", "channel": "general"}
    receipt = host.threads.receipt(scope, "repair-1")
    write_json(host.home.state / "requests" / f"{receipt['request_id']}.json", receipt["request"])
    calls = len(host.session.fake.requests)
    host.tick()
    assert len(host.session.fake.requests) == calls
    assert host.backlog.get(task_id)["thread_id"] == result["thread_id"]
