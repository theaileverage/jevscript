"""An offline stand-in for Jev: a localhost TypeSafe-shaped endpoint.

The runtime reaches Jev through the endpoint in the selected model profile
(spec section 10.6), so pointing a profile at this server runs the real SDK,
runtime, compiler and recordings with no network and no key. Answers come
from scripted rules first, then from a small, deterministic heuristic:

- a machine step picks the enabled event whose description shares the most
  words with the observation, preferring any move over waiting;
- a ``pick`` or ``pick among`` picks the option whose description shares the
  most words with the text being judged (the escape option when none does,
  for ``pick among``; the first label for ``pick``);
- ``feels`` answers 0.15 unless a rule says otherwise;
- ``rate`` answers the second level.

Rules are ``{"match": {...}, "answer": {...}}``. ``match`` keys, all optional
and all regexes: ``id`` (question id), ``question`` (instruction text),
``inspect`` (state path), ``text`` (the judged value's text form). ``answer``
is ``{"noul": p}``, ``{"choice": label}`` (the label may be a regex; an item
label ``i<n>`` can be picked by ``{"item": regex}`` over the option texts), or
``{"score": s}``. Every request is kept in ``requests`` for assertions.
"""

from __future__ import annotations

import contextlib
import json
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterator

MODEL = "cos-offline"

STOP = set(
    "a an and are as at be by for from has have in is it its of on or that the this to was were will with "
    "about any asks change changes do does not no one only other these those which who why".split()
)


def words(text: str) -> set[str]:
    out = set()
    for word in re.findall(r"[a-z0-9]+", text.lower()):
        if word in STOP or len(word) < 3:
            continue
        out.add(word[:5])  # a crude stem: "documentation" and "docs" still miss, "tests"/"testing" meet
    return out


def resolve(state: Any, path: str) -> Any:
    value = state
    for part in re.findall(r"[^.\[\]]+|\[\d+\]", path):
        if part.startswith("["):
            index = int(part[1:-1])
            value = value[index] if isinstance(value, list) and index < len(value) else None
        else:
            value = value.get(part) if isinstance(value, dict) else None
    return value


def text_of(value: Any) -> str:
    return value if isinstance(value, str) else json.dumps(value)


def _criterion_text(value: Any) -> str:
    return value if isinstance(value, str) else " ".join(str(v) for v in value.values()) if isinstance(value, dict) else str(value)


class FakeJev:
    def __init__(self, rules: list[dict[str, Any]] | None = None) -> None:
        self.rules = list(rules or [])
        self.requests: list[dict[str, Any]] = []
        self._server: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None

    # -- answering --------------------------------------------------------------

    def _rule_for(self, qid: str, question: dict[str, Any], judged: str) -> dict[str, Any] | None:
        instructions = question.get("instructions", {})
        fields = {"id": qid, "question": instructions.get("question", ""), "inspect": instructions.get("inspect", ""), "text": judged}
        for rule in self.rules:
            match = rule.get("match", {})
            if all(re.search(pattern, fields.get(key, ""), re.IGNORECASE | re.DOTALL) for key, pattern in match.items()):
                return rule["answer"]
        return None

    def answer(self, qid: str, question: dict[str, Any], state: dict[str, Any]) -> dict[str, Any]:
        inspect = question.get("instructions", {}).get("inspect", "")
        judged_value = resolve(state, inspect)
        judged = text_of(judged_value)
        # A pick among reads the request from its batch-mates in the state.
        context = " ".join(text_of(v) for k, v in state.items() if k != inspect.split(".")[0].split("[")[0])
        rule = self._rule_for(qid, question, judged)
        kind = question["type"]
        if kind == "noul":
            return {"noul": float((rule or {}).get("noul", 0.15))}
        if kind == "score":
            levels = len(question.get("criteria", []))
            score = float((rule or {}).get("score", min(1, levels - 1)))
            nearest = max(0, min(levels - 1, round(score)))
            probabilities = {str(i): (0.8 if i == nearest else 0.2 / max(1, levels - 1)) for i in range(levels)}
            return {"score": score, "probabilities": probabilities}
        criteria: dict[str, Any] = question.get("criteria", {})
        labels = list(criteria)
        chosen = None
        if rule is not None and "choice" in rule:
            chosen = next((l for l in labels if re.fullmatch(rule["choice"], l)), None)
        if rule is not None and "item" in rule:
            chosen = next((l for l in labels if re.search(rule["item"], _criterion_text(criteria[l]), re.IGNORECASE)), None)
        if chosen is None:
            chosen = self._heuristic(qid, labels, criteria, judged if qid == "event" or not labels or not labels[0].startswith("i") else context)
        spread = 0.15 / max(1, len(labels) - 1)
        probabilities = {l: (0.85 if l == chosen else spread) for l in labels}
        return {"choice": chosen, "probabilities": probabilities, "confidence": float((rule or {}).get("confidence", 0.8))}

    def _heuristic(self, qid: str, labels: list[str], criteria: dict[str, Any], text: str) -> str:
        if qid == "event":
            # A machine step: the enabled event whose description best matches
            # the observation, preferring any real move over waiting.
            moves = [l for l in labels if l != "stay"]
            waiting = re.compile(r"\bnothing\b|not finished|still running|busy", re.IGNORECASE)
            active = [l for l in moves if not waiting.search(_criterion_text(criteria[l]))] or moves
            if not active:
                return "stay"
            target = words(text)
            return max(active, key=lambda l: (len(words(_criterion_text(criteria[l]) + " " + l) & target), -active.index(l)))
        escape = next((l for l in labels if l in ("other", "none")), None)
        target = words(text)
        scored = [(len(words(_criterion_text(criteria[l]) + " " + l) & target), -i, l) for i, l in enumerate(labels) if l != escape]
        best = max(scored) if scored else (0, 0, escape)
        if best[0] > 0:
            return best[2]
        if labels and labels[0].startswith("i"):
            return escape or labels[0]
        return labels[0]

    def respond(self, request: dict[str, Any]) -> dict[str, Any]:
        self.requests.append(request)
        answers = {qid: self.answer(qid, q, request.get("state", {})) for qid, q in request.get("questions", {}).items()}
        return {"model": MODEL, "answers": answers, "usage": {"input_tokens": len(json.dumps(request)) // 4, "output_tokens": 0}}

    # -- serving ----------------------------------------------------------------

    def start(self) -> "FakeJev":
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:  # noqa: N802 - stdlib hook
                body = json.loads(self.rfile.read(int(self.headers.get("content-length", "0"))))
                payload = json.dumps(fake.respond(body)).encode()
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *_: Any) -> None:
                return

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self

    @property
    def endpoint(self) -> str:
        assert self._server is not None
        return f"http://127.0.0.1:{self._server.server_port}/v1/systemone"

    def profile(self) -> dict[str, Any]:
        return {
            "model": MODEL,
            "endpoint": self.endpoint,
            "total_tokens": 64000,
            "state_plus_question_tokens": 32000,
            "max_questions_per_request": 64,
            "max_criteria_per_question": 255,
            "tokenizer": "chars4",
            "price_per_million_input_usd": 0.0,
            "price_per_million_output_usd": 0.0,
        }

    def write_profiles(self, path: Path) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps([self.profile()]), encoding="utf-8")
        return path

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None


@contextlib.contextmanager
def serving(rules: list[dict[str, Any]] | None = None) -> Iterator[FakeJev]:
    fake = FakeJev(rules).start()
    try:
        yield fake
    finally:
        fake.stop()
