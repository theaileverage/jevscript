#!/usr/bin/env python3
"""Run the Jevscript skill-suggestion cookbook live, scripted, or in replay."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterator

from project_env import load_project_env


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
PROGRAM = HERE / "skill_suggestion.jev"
ROSTER_PATH = HERE / "data" / "hermes_roster.json"
LICENSES_PATH = HERE / "data" / "hermes_roster.licenses.json"
SAMPLES_PATH = HERE / "data" / "sample_requests.json"
SCRIPTED_PATH = HERE / "fixtures" / "scripted_answers.json"
BINARY = REPO / "target" / "debug" / "jevscript"
ROSTER_SHA256 = "86eeeae1fe39a2b0fcceb2d78bcaf8c4a77d3973510876860be064bf35a85405"
LICENSES_SHA256 = "31115b4806fc6b3474ea823504c0de4ba1e56fb35ea5e079f9a6504efe1fefa9"
ROSTER_COUNT = 157
CATEGORY_COUNT = 29
ALLOWED_LICENSES = frozenset({"MIT", "Apache-2.0"})

load_project_env(REPO)

sys.path.insert(0, str(REPO / "sdk" / "python" / "src"))
from jevscript import load  # noqa: E402


def load_roster() -> list[dict[str, Any]]:
    payload = ROSTER_PATH.read_bytes()
    digest = hashlib.sha256(payload).hexdigest()
    if digest != ROSTER_SHA256:
        raise ValueError(f"roster checksum drift: expected {ROSTER_SHA256}, got {digest}")
    license_payload = LICENSES_PATH.read_bytes()
    license_digest = hashlib.sha256(license_payload).hexdigest()
    if license_digest != LICENSES_SHA256:
        raise ValueError(
            f"license manifest checksum drift: expected {LICENSES_SHA256}, got {license_digest}"
        )
    roster = json.loads(payload)
    licenses = json.loads(license_payload)
    if len(roster) != ROSTER_COUNT:
        raise ValueError(f"roster count drift: expected {ROSTER_COUNT}, got {len(roster)}")
    if len({skill["category"] for skill in roster}) != CATEGORY_COUNT:
        raise ValueError("roster category-count drift")
    names = [skill["name"] for skill in roster]
    if len(names) != len(set(names)) or set(names) != set(licenses):
        raise ValueError("roster and license manifest must have the same unique names")
    if any(license_name not in ALLOWED_LICENSES for license_name in licenses.values()):
        raise ValueError("roster contains a skill without a permitted license declaration")
    return [{**skill, "label": f"i{index}"} for index, skill in enumerate(roster)]


def load_samples() -> list[dict[str, Any]]:
    return json.loads(SAMPLES_PATH.read_text(encoding="utf-8"))


def _run_task(
    request: str,
    *,
    model: str,
    profiles: str | None = None,
    record: str | None = None,
    replay: str | None = None,
) -> dict[str, Any]:
    if not BINARY.exists():
        raise FileNotFoundError(f"build the runtime first: cargo build (missing {BINARY})")
    inputs = {"request": request, "roster": load_roster()}
    program = load(str(PROGRAM), bin=str(BINARY), cwd=str(REPO))
    try:
        run = program.task("main").start(
            inputs=inputs,
            model=model if replay is None else None,
            profiles=profiles if replay is None else None,
            record=record,
            replay=replay,
        )
        pause = run.next()
        if pause["kind"] != "done":
            raise RuntimeError(f"unexpected pause: {json.dumps(pause, ensure_ascii=False)}")
        return pause
    finally:
        program.close()
        # The current SDK waits for the child but leaves its read pipes open.
        # Close them here so cookbook test runs do not leak file descriptors.
        process = program._client._process  # type: ignore[attr-defined]
        for stream in (process.stdout, process.stderr):
            if stream is not None:
                stream.close()


def live(request: str, model: str, record: str | None) -> dict[str, Any]:
    if not os.environ.get("TYPESAFE_API_KEY"):
        raise SystemExit("TYPESAFE_API_KEY is required for a live run")
    return _run_task(request, model=model, record=record)


class _ScriptedServer(ThreadingHTTPServer):
    scenario: dict[str, Any]
    requests_seen: list[dict[str, Any]]


def _probabilities(labels: list[str], ranked: list[str], state_items: list[dict[str, Any]]) -> dict[str, float]:
    probabilities = {label: 0.0 for label in labels}
    weights = (0.70, 0.20, 0.10)
    by_name = {item["name"]: f"i{index}" for index, item in enumerate(state_items)}
    for name, weight in zip(ranked, weights):
        label = by_name.get(name)
        if label in probabilities:
            probabilities[label] = weight
    total = sum(probabilities.values())
    if total == 0:
        probabilities[labels[0]] = 1.0
    elif total < 1:
        probabilities[next(label for label in labels if probabilities[label] > 0)] += 1 - total
    return probabilities


class _ScriptedHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802 - stdlib hook name
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        server = self.server
        assert isinstance(server, _ScriptedServer)
        server.requests_seen.append(request)
        answers: dict[str, Any] = {}
        scenario = server.scenario
        for question_id, question in request["questions"].items():
            if question["type"] == "choice":
                labels = list(question["criteria"])
                inspect = question["instructions"]["inspect"]
                state_items = request["state"][inspect]
                if inspect == "wide_roster":
                    ranked = scenario["wide"]
                    selected_name = ranked[0]
                else:
                    selected_name = scenario["rerank"]
                    ranked = [selected_name] + [
                        item["name"] for item in state_items if item["name"] != selected_name
                    ]
                probabilities = _probabilities(labels, ranked, state_items)
                selected_index = next(
                    index for index, item in enumerate(state_items) if item["name"] == selected_name
                )
                answers[question_id] = {
                    "type": "choice",
                    "choice": f"i{selected_index}",
                    "confidence": max(probabilities.values()),
                    "probabilities": probabilities,
                }
                continue

            text = question["instructions"]["question"]
            if "act on the user's files" in text:
                value = scenario["gate"]["acts_on_user_system"]
            elif "consult a specific documented procedure" in text:
                value = scenario["gate"]["would_follow_documented_procedure"]
            elif "fully satisfy this request in prose" in text:
                value = scenario["gate"]["prose_suffices"]
            else:
                match = re.fullmatch(r"detailed\[(\d+)]", question["instructions"]["inspect"])
                if not match:
                    raise AssertionError(f"unrecognized scripted question: {question}")
                candidate = request["state"]["detailed"][int(match.group(1))]["name"]
                value = scenario["fits"][candidate]
            answers[question_id] = {"type": "noul", "noul": value}

        payload = json.dumps(
            {
                "model": "offline-scripted",
                "answers": answers,
                "usage": {"input_tokens": 1, "output_tokens": 0},
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, _format: str, *_args: object) -> None:
        return


@contextlib.contextmanager
def scripted_endpoint(scenario: dict[str, Any], directory: Path) -> Iterator[_ScriptedServer]:
    server = _ScriptedServer(("127.0.0.1", 0), _ScriptedHandler)
    server.scenario = scenario
    server.requests_seen = []
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    profiles = [
        {
            "model": "offline-scripted",
            "endpoint": f"http://127.0.0.1:{server.server_port}/v1/systemone",
            "total_tokens": 64000,
            "state_plus_question_tokens": 32000,
            "max_questions_per_request": 64,
            "max_criteria_per_question": 255,
            "tokenizer": "chars4",
            "price_per_million_input_usd": 0.0,
            "price_per_million_output_usd": 0.0,
        }
    ]
    profile_path = directory / "profiles.json"
    profile_path.write_text(json.dumps(profiles), encoding="utf-8")
    old_key = os.environ.get("TYPESAFE_API_KEY")
    os.environ["TYPESAFE_API_KEY"] = "offline-scripted-fixture"
    server.profile_path = profile_path  # type: ignore[attr-defined]
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        if old_key is None:
            os.environ.pop("TYPESAFE_API_KEY", None)
        else:
            os.environ["TYPESAFE_API_KEY"] = old_key


def scripted_case(case_id: str, *, record: str | None = None) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    samples = {case["id"]: case for case in load_samples()}
    scenarios = json.loads(SCRIPTED_PATH.read_text(encoding="utf-8"))
    case = samples[case_id]
    with tempfile.TemporaryDirectory(prefix="skill-suggestion-") as temp:
        with scripted_endpoint(scenarios[case_id], Path(temp)) as server:
            pause = _run_task(
                case["text"],
                model="offline-scripted",
                profiles=str(server.profile_path),  # type: ignore[attr-defined]
                record=record,
            )
            return pause, list(server.requests_seen)


def verify() -> list[dict[str, Any]]:
    results = []
    for case in load_samples():
        pause, requests = scripted_case(case["id"])
        output = pause["outputs"]
        if output["suggestion"] != case["gold"]:
            raise AssertionError(
                f"{case['id']}: expected {case['gold']!r}, got {output['suggestion']!r}"
            )
        expected_requests = 1 if case["id"] == "prose_only" else 2
        if len(requests) != expected_requests:
            raise AssertionError(
                f"{case['id']}: expected {expected_requests} request groups, got {len(requests)}"
            )
        results.append(
            {
                "case": case["id"],
                "suggestion": output["suggestion"],
                "gate_score": output["gate_score"],
                "shortlist": output["shortlist"],
                "request_groups": len(requests),
                "questions_per_request": [len(request["questions"]) for request in requests],
                "evidence": "fixture/scripted; no external network calls",
            }
        )
    return results


def replay(recording: Path, request: str) -> dict[str, Any]:
    return _run_task(request, model="jev-latest", replay=str(recording))


def _main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    live_parser = sub.add_parser("live", help="run two paid TypeSafe requests when the gate passes")
    live_parser.add_argument("--request", required=True)
    live_parser.add_argument("--model", default="jev-latest")
    live_parser.add_argument("--record")
    scripted_parser = sub.add_parser("scripted", help="run one named offline fixture through the real SDK/runtime")
    scripted_parser.add_argument("--case", choices=[case["id"] for case in load_samples()], required=True)
    scripted_parser.add_argument("--record")
    sub.add_parser("verify", help="run all four deterministic offline fixtures")
    replay_parser = sub.add_parser("replay", help="replay a recording with zero model calls")
    replay_parser.add_argument("recording", type=Path)
    replay_parser.add_argument("--request", required=True)
    args = parser.parse_args()

    if args.command == "live":
        result = live(args.request, args.model, args.record)
    elif args.command == "scripted":
        pause, requests = scripted_case(args.case, record=args.record)
        result = {
            "pause": pause,
            "request_groups": len(requests),
            "questions_per_request": [len(item["questions"]) for item in requests],
            "evidence": "fixture/scripted; no external network calls",
        }
    elif args.command == "verify":
        result = verify()
    else:
        result = replay(args.recording, args.request)
    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    _main()
