"""The Python SDK against the real ``jevscript serve`` boundary."""

from __future__ import annotations

import json
import os
import pathlib
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import pytest

from jevscript import METHODS, JevscriptRpcError, RpcClient, load

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
BINARY = os.environ.get("JEVSCRIPT_BIN") or str(REPO_ROOT / "target" / "debug" / "jevscript")

needs_binary = pytest.mark.skipif(
    not pathlib.Path(BINARY).exists(),
    reason="the runtime binary is not built; run `cargo build`",
)


@pytest.fixture()
def client():
    with RpcClient(bin=BINARY, cwd=str(REPO_ROOT)) as rpc:
        yield rpc


@needs_binary
def test_program_load_returns_metadata_and_unknown_methods_are_rejected(client):
    loaded = client.request(
        METHODS["program_load"],
        {"path": "examples/inbox_triage.jev", "paths": []},
    )
    assert loaded["name"] == "inbox_triage"
    assert [item["name"] for item in loaded["inputs"]] == ["message"]
    assert "triage" in [item["name"] for item in loaded["judgments"]]
    assert "span" not in loaded["inputs"][0]
    judgment = loaded["judgments"][0]
    assert "span" not in judgment
    assert judgment["results"][0] == {
        "name": "urgent",
        "each": False,
        "verb": "feels",
    }
    assert judgment["results"][1]["verb"] == "pick"
    assert judgment["results"][1]["labels"] == [
        "code",
        "customer",
        "schedule",
        "other",
    ]
    assert "request_group" not in json.dumps(loaded["judgments"])
    assert "subject" not in json.dumps(loaded["judgments"])

    with pytest.raises(JevscriptRpcError) as caught:
        client.request("program.destroy", {})
    assert caught.value.code == -32601


@needs_binary
def test_path_load_preserves_the_diagnostic_filename(tmp_path):
    invalid = tmp_path / "uppercase.jev"
    invalid.write_text("program INVALID\n", encoding="utf-8")
    with pytest.raises(JevscriptRpcError) as caught:
        load(str(invalid), bin=BINARY, cwd=str(REPO_ROOT))
    assert caught.value.data["kind"] == "compile_error"
    assert caught.value.data["diagnostics"][0]["code"] == "uppercase_identifier"
    assert caught.value.data["diagnostics"][0]["file"] == str(invalid)


@needs_binary
def test_record_and_replay_are_mutually_exclusive(tmp_path):
    source = "program exclusive\n\ntask main:\n  return true\n"
    with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        with pytest.raises(JevscriptRpcError) as caught:
            program.task("main").start(
                record=str(tmp_path / "new.jsonl"),
                replay=str(tmp_path / "missing.jsonl"),
            )
    assert caught.value.data["kind"] == "type_error"


@needs_binary
def test_only_manifest_declared_tool_handles_are_encoded():
    class HandleTool:
        kind = "tool"
        capability: str | None = None

        def __init__(self) -> None:
            self.inspected: Any = None

        def call(
            self, verb: str, args: dict[str, Any], capability: str | None = None
        ) -> Any:
            assert capability == "tree"
            if verb == "create":
                return {"id": "tree-1"}
            if verb == "inspect":
                self.inspected = args["positional"][0]
                return True
            return {"id": "plain-record"}

    source = """program handle_wire

out metadata

needs tree: tool:
  create(name) -> handle
  inspect(item) -> bool
  metadata() -> record

task main:
  item = tree.create "branch"
  tree.inspect item
  metadata = tree.metadata
"""
    tool = HandleTool()
    with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        pause = program.task("main").start(bind={"tree": tool}).next()
    assert pause["kind"] == "done"
    assert pause["outputs"]["metadata"] == {"id": "plain-record"}
    assert tool.inspected == {
        "$jev": "handle",
        "capability": "tree",
        "id": "tree-1",
    }


class Tool:
    kind = "tool"

    def __init__(self) -> None:
        self.capability: str | None = None
        self.seen_capability: str | None = None

    def call(self, _verb: str, _args: dict[str, Any], capability: str | None = None) -> None:
        self.seen_capability = capability


@needs_binary
def test_task_calls_bound_adapter_streams_events_and_replays(tmp_path):
    source = "program sdk\n\nneeds tree: tool\n\ntask main:\n  tree.touch\n"
    tool = Tool()
    recording = tmp_path / "run.jsonl"
    with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        run = program.task("main").start(bind={"tree": tool}, record=str(recording))
        pauses = list(run)
        assert pauses[-1]["kind"] == "done"
        assert tool.capability == "tree"
        assert tool.seen_capability == "tree"
        assert any(event["event"] == "call" for event in run.events())

        replay = program.task("main").start(replay=str(recording))
        assert list(replay)[-1]["kind"] == "done"
        assert tool.seen_capability == "tree", "replay made no new adapter call"


@needs_binary
def test_each_run_keeps_its_own_adapter_binding():
    class NamedTool:
        kind = "tool"
        capability: str | None = None

        def __init__(self, name: str) -> None:
            self.name = name
            self.calls = 0

        def call(
            self, _verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> None:
            assert capability == "tree"
            self.calls += 1

    source = "program scoped\n\nneeds tree: tool\n\ntask main:\n  tree.touch\n"
    first_tool = NamedTool("first")
    second_tool = NamedTool("second")
    with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        first = program.task("main").start(bind={"tree": first_tool})
        second = program.task("main").start(bind={"tree": second_tool})
        assert first.next()["kind"] == "done"
        assert second.next()["kind"] == "done"
        assert first_tool.calls == 1
        assert second_tool.calls == 1


class RetryableAdapterError(RuntimeError):
    retryable = True


@needs_binary
def test_adapter_retryability_reaches_the_runtime_pause():
    class FailingTool:
        kind = "tool"
        capability: str | None = None

        def call(
            self, _verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> None:
            assert capability == "tree"
            raise RetryableAdapterError("try later")

    source = "program retry\n\nneeds tree: tool\n\ntask main:\n  tree.fail\n"
    with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        run = program.task("main").start(bind={"tree": FailingTool()})
        pause = run.next()
        assert pause["kind"] == "error"
        assert pause["code"] == "adapter_error"
        assert pause["retryable"] is True
        run.abort()


@needs_binary
def test_iteration_ends_on_default_terminals_and_continues_resumed_escalation():
    source = """program terminals

out continued

task main:
  escalate "help"
  continued = true
"""
    with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        assert [pause["kind"] for pause in program.task("main").start()] == [
            "escalate"
        ]

        resumed_run = program.task("main").start()
        resumed = []
        for pause in resumed_run:
            resumed.append(pause["kind"])
            if pause["kind"] == "escalate":
                resumed_run.resume({"resume": True})
        assert resumed == ["escalate", "done"]

    class FailingTool:
        kind = "tool"
        capability: str | None = None

        def call(
            self, _verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> None:
            assert capability == "tree"
            raise RuntimeError("permanent")

    error_source = (
        "program error_terminal\n\nneeds tree: tool\n\ntask main:\n  tree.fail\n"
    )
    with load(
        error_source, source=True, bin=BINARY, cwd=str(REPO_ROOT)
    ) as program:
        errors = [
            pause["kind"]
            for pause in program.task("main").start(bind={"tree": FailingTool()})
        ]
        assert errors == ["error"]


@needs_binary
def test_load_paths_apply_during_linking(tmp_path):
    (tmp_path / "library.jev").write_text(
        "program library\n\ndef ok():\n  return true\n", encoding="utf-8"
    )
    source = (
        'program rooted\n\nuse "library.jev" as lib\n\ntask main:\n  return lib.ok()\n'
    )
    with load(
        source,
        source=True,
        paths=[str(tmp_path)],
        bin=BINARY,
        cwd=str(REPO_ROOT),
    ) as program:
        assert program.name == "rooted"


class JevHandler(BaseHTTPRequestHandler):
    machine_choices: list[str] = []
    requests = 0
    choice = "needs_me"
    confidence = 1.0
    off_scope = 0.9

    def do_POST(self) -> None:  # noqa: N802 - stdlib hook name
        type(self).requests += 1
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        answers: dict[str, Any] = {}
        for name, question in request["questions"].items():
            if question["type"] == "noul":
                if name == "same":
                    noul = 0.1
                elif name.startswith("off_scope"):
                    noul = type(self).off_scope
                else:
                    noul = 0.9
                answers[name] = {"type": "noul", "noul": noul}
            elif question["type"] == "choice":
                labels = list(question["criteria"])
                wanted = (
                    type(self).machine_choices.pop(0)
                    if name == "event" and type(self).machine_choices
                    else type(self).choice
                )
                selected = wanted if wanted in labels else labels[0]
                answers[name] = {
                    "type": "choice",
                    "choice": selected,
                    "confidence": type(self).confidence,
                    "probabilities": {
                        label: 1.0 if label == selected else 0.0 for label in labels
                    },
                }
            else:
                levels = question["criteria"]
                score = len(levels) - 1 if name == "progress" else 0
                answers[name] = {
                    "type": "score",
                    "score": score,
                    "confidence": 1.0,
                    "probabilities": {
                        str(index): 1.0 if index == score else 0.0
                        for index, _ in enumerate(levels)
                    },
                }
        body = json.dumps(
            {
                "model": "test",
                "answers": answers,
                "usage": {"input_tokens": 1, "output_tokens": 0},
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def start_jev(tmp_path, monkeypatch):
    JevHandler.machine_choices = []
    JevHandler.requests = 0
    JevHandler.choice = "needs_me"
    JevHandler.confidence = 1.0
    JevHandler.off_scope = 0.9
    server = ThreadingHTTPServer(("127.0.0.1", 0), JevHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("TYPESAFE_API_KEY", "test-key")
    profiles = tmp_path / "profiles.json"
    profiles.write_text(
        json.dumps(
            [
                {
                    "model": "test",
                    "endpoint": f"http://127.0.0.1:{server.server_port}/v1/systemone",
                    "total_tokens": 64000,
                    "state_plus_question_tokens": 32000,
                    "max_questions_per_request": 64,
                    "max_criteria_per_question": 64,
                    "tokenizer": "chars4",
                    "price_per_million_input_usd": 0,
                    "price_per_million_output_usd": 0,
                }
            ]
        ),
        encoding="utf-8",
    )
    return server, thread, profiles


@needs_binary
def test_judgment_profiles_option_reaches_a_local_endpoint(tmp_path, monkeypatch):
    server, thread, profiles = start_jev(tmp_path, monkeypatch)
    try:
        with load(
            "examples/inbox_triage.jev", bin=BINARY, cwd=str(REPO_ROOT)
        ) as program:
            answers = program.judgment("triage").run(
                {"message": "please fix this bug"},
                model="test",
                profiles=str(profiles),
            )
            assert answers["owner"]["$jev"] == "choice"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


@needs_binary
def test_fix_issue_confirms_escalates_and_replays_without_effects(tmp_path, monkeypatch):
    server, thread, profiles = start_jev(tmp_path, monkeypatch)
    calls: list[str] = []

    class Agent:
        kind = "agent"
        capability: str | None = None

        def call(
            self, verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> Any:
            calls.append(f"{capability}.{verb}")
            if verb == "spawn":
                return {"id": "dev-1"}
            if verb == "wait":
                return self.observation()
            return None

        def observe(
            self, _handle: dict[str, Any], capability: str | None = None
        ) -> dict[str, Any]:
            calls.append(f"{capability}.observe")
            return self.observation()

        @staticmethod
        def observation() -> dict[str, Any]:
            return {
                "status": "waiting",
                "last_message": "done",
                "tail": "done",
                "exit_code": None,
            }

    class Tree:
        kind = "tool"
        capability: str | None = None

        def __init__(self) -> None:
            self.passing = False

        def call(
            self, verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> Any:
            calls.append(f"{capability}.{verb}")
            if verb == "diff":
                return {"files": ["src/lib.rs"]}
            if verb == "test_summary":
                return "passing soon"
            if verb == "tests_pass":
                return self.passing
            if verb == "open_pr":
                return "https://example.test/pr/1"
            return None

    class Person:
        kind = "person"
        capability: str | None = None

        def call(
            self, verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> None:
            calls.append(f"{capability}.{verb}")

    inputs = {"issue": {"title": "bug", "body": "fix it", "branch": "fix"}}
    recording = tmp_path / "fix-issue.jsonl"
    try:
        with load("examples/fix_issue.jev", bin=BINARY, cwd=str(REPO_ROOT)) as program:
            tree = Tree()
            run = program.task("main").start(
                inputs=inputs,
                bind={"claude": Agent(), "tree": tree, "me": Person()},
                record=str(recording),
                model="test",
                profiles=str(profiles),
            )
            pauses = []
            for pause in run:
                pauses.append(pause)
                if pause["kind"] == "confirm":
                    tree.passing = True
                    run.resume({"answer": "yes", "text": "continue"})
            assert any(pause["kind"] == "confirm" for pause in pauses), pauses
            assert pauses[-1]["kind"] == "done"
            assert pauses[-1]["outputs"]["pr_url"] == "https://example.test/pr/1"
            events = list(run.events())
            assert any(event["event"] == "request" for event in events)
            assert any(event["event"] == "call" for event in events)

            live_calls = list(calls)
            live_requests = JevHandler.requests
            replayed = list(program.task("main").start(replay=str(recording)))
            assert replayed == pauses
            assert calls == live_calls
            assert JevHandler.requests == live_requests

            JevHandler.choice = "keep_working"
            JevHandler.confidence = 0.1
            JevHandler.off_scope = 0.1
            escalated = program.task("main").start(
                inputs=inputs,
                bind={"claude": Agent(), "tree": Tree(), "me": Person()},
                model="test",
                profiles=str(profiles),
            )
            assert escalated.next()["kind"] == "waiting"
            assert escalated.next()["kind"] == "escalate"
            escalated.abort()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


@needs_binary
def test_review_loop_machine_runs_verified_and_replays_without_effects(tmp_path, monkeypatch):
    server, thread, profiles = start_jev(tmp_path, monkeypatch)
    JevHandler.machine_choices = ["finished", "approved"]
    calls: list[str] = []

    class Agent:
        kind = "agent"
        capability: str | None = None

        def call(
            self, verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> Any:
            calls.append(f"{capability}.{verb}")
            if verb == "spawn":
                return {"id": "reviewer-1"}
            return None

        def observe(
            self, _handle: dict[str, Any], capability: str | None = None
        ) -> dict[str, Any]:
            calls.append(f"{capability}.observe")
            return {
                "status": "waiting",
                "last_message": "ready",
                "tail": "",
                "exit_code": None,
            }

    class Tree:
        kind = "tool"
        capability: str | None = None

        def call(
            self, verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> Any:
            calls.append(f"{capability}.{verb}")
            if verb == "tests_pass":
                return True
            if verb == "test_summary":
                return "passing"
            return None

    class Person:
        kind = "person"
        capability: str | None = None

        def call(
            self, verb: str, _args: dict[str, Any], capability: str | None = None
        ) -> None:
            calls.append(f"{capability}.{verb}")

    recording = tmp_path / "review.jsonl"
    try:
        with load("examples/review_loop.jev", bin=BINARY, cwd=str(REPO_ROOT)) as program:
            run = program.task("main").start(
                bind={"claude": Agent(), "tree": Tree(), "me": Person()},
                record=str(recording),
                model="test",
                profiles=str(profiles),
            )
            pauses = list(run)
            assert pauses[-1]["kind"] == "done"
            machine_steps = [event for event in run.events() if event["event"] == "machine_step"]
            assert len(machine_steps) == 2
            assert machine_steps[-1]["chosen"] == "approved"
            assert machine_steps[-1]["to"] == "approved"
            assert calls[-1] == "me.notify"

            live_calls = list(calls)
            live_requests = JevHandler.requests
            replay = program.task("main").start(replay=str(recording))
            replayed = replay.next()
            assert replayed == pauses[-1]
            assert calls == live_calls
            assert JevHandler.requests == live_requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


LOG_SOURCE = (
    "program logs\n\nin message: text\nout n\n\n"
    "task main:\n"
    '  log info "routed request" { message }\n'
    "  n = log debug len(message)\n"
)


@needs_binary
def test_run_hands_each_log_to_on_log_and_a_replay_does_not_reemit_them(tmp_path):
    # Spec 5.8: every log is a typed line the host can route; a replay checks
    # the recorded lines without emitting them again.
    recording = tmp_path / "run.jsonl"
    seen: list[dict[str, Any]] = []
    with load(LOG_SOURCE, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        run = program.task("main").start(
            inputs={"message": "hello"}, record=str(recording), on_log=seen.append
        )
        pauses = list(run)
        assert pauses[-1]["kind"] == "done"
        assert pauses[-1]["outputs"]["n"] == 5
        assert [(line["level"], line["message"]) for line in seen] == [
            ("info", "routed request"),
            ("debug", "5"),
        ]
        assert seen[0]["fields"] == {"message": "hello"}
        assert seen[1]["fields"] == {}
        assert all(line["task"] == "main" and line["run_id"] == run.id for line in seen)
        assert seen[0]["source"]["start"]["line"] == 7
        assert list(run.logs()) == seen

        replayed: list[dict[str, Any]] = []
        replay = program.task("main").start(replay=str(recording), on_log=replayed.append)
        assert list(replay)[-1]["kind"] == "done"
        assert replayed == [], "a replayed log is not emitted to the host again"
        assert list(replay.logs()) == []


@needs_binary
def test_a_redacted_recording_stores_log_markers_while_the_host_sees_full_lines(tmp_path):
    # Spec 5.8 and 10.3: redaction governs the stored recording.
    recording = tmp_path / "run.jsonl"
    seen: list[dict[str, Any]] = []
    with load(LOG_SOURCE, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
        run = program.task("main").start(
            inputs={"message": "secret"},
            record=str(recording),
            redact=True,
            on_log=seen.append,
        )
        assert list(run)[-1]["kind"] == "done"
    assert seen[0]["fields"] == {"message": "secret"}
    logged = [
        json.loads(line)
        for line in recording.read_text(encoding="utf-8").splitlines()
        if '"event":"log"' in line
    ]
    assert len(logged) == 2
    assert set(logged[0]["message"]) == {"redacted"}
    assert set(logged[0]["fields"]["message"]) == {"redacted"}
    assert "secret" not in json.dumps(logged)


@needs_binary
def test_a_judgment_run_alone_hands_its_logs_to_on_log(tmp_path, monkeypatch):
    # Spec 5.8 and 11.3: no run, no recording; the lines come back with the
    # answers, named after the judgment.
    server, thread, profiles = start_jev(tmp_path, monkeypatch)
    source = (
        "program triage\n\njudgment classify(message):\n"
        '  urgent = message feels "is urgent"\n'
        '  log warn "answered" { urgent }\n'
    )
    seen: list[dict[str, Any]] = []
    try:
        with load(source, source=True, bin=BINARY, cwd=str(REPO_ROOT)) as program:
            answers = program.judgment("classify").run(
                {"message": "now"}, model="test", profiles=str(profiles), on_log=seen.append
            )
            assert answers["urgent"] == {"$jev": "prob", "value": 0.9}
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    assert len(seen) == 1
    assert seen[0]["level"] == "warn"
    assert seen[0]["message"] == "answered"
    assert seen[0]["task"] == "classify"
    assert seen[0]["fields"] == {"urgent": {"$jev": "prob", "value": 0.9}}
