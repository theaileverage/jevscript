"""The episode runner: one wake, one bounded Jevscript run.

Jevscript 0.1 bounds every loop and a run's calls, cannot resume a run that
crashed in the middle of an effect, and recovers only once after a restart.
So the Chief of Staff never holds a long-lived run. For each wake the host
builds a snapshot, starts one task of ``cos.jev`` with it as input, records
the run to a new JSONL file, and returns the task's ``result`` output for the
host to persist. The wake is acknowledged only after that.

Pauses never block the watcher:

- ``confirm`` (``me.ask``): the question becomes a durable decision and the
  run is parked. While the host lives, the run waits in memory; after a
  restart it is rebuilt by replaying its recording with zero model calls up to
  the open question. Either way the person's answer resumes it, and the rest
  of the run continues live (spec section 10.4).
- ``escalate`` and ``budget``: recorded as a decision and the run ends.
- a retryable ``error`` is retried twice; any other error ends the run.

``JevSession`` owns which Jev the runs talk to: ``live`` (TypeSafe, key from
the keychain) or ``fake`` (``fakejev.FakeJev`` on localhost, no key).
"""

from __future__ import annotations

import contextlib
import json
import os
import secrets
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator

from .fakejev import FakeJev
from .jevbin import JEV_KEY_ENV, SetupError, find_jevscript, keychain_key, load
from .state.home import Home, iso, now, read_json, write_json


class JevSession:
    """Which Jev a home talks to, and how programs are loaded against it."""

    def __init__(self, home: Home, mode: str | None = None, rules: list[dict[str, Any]] | None = None) -> None:
        self.home = home
        config = home.config["jev"]
        self.mode = mode or os.environ.get("COS_JEV") or config["mode"]
        self.model = config["model"]
        self.service = config["keychain_service"]
        self.account = config.get("keychain_account")
        self.fake: FakeJev | None = None
        self.profiles: str | None = None
        self._key: str | None = None
        if self.mode == "fake":
            rules = rules if rules is not None else self._rules_from_config()
            self.fake = FakeJev(rules).start()
            self.profiles = str(self.fake.write_profiles(home.state / "fake-profiles.json"))
            self.model = self.fake.profile()["model"]
        elif self.mode != "live":
            raise SetupError(f"jev.mode must be `live` or `fake`, not `{self.mode}`")

    def _rules_from_config(self) -> list[dict[str, Any]]:
        path = self.home.config["jev"].get("fake_rules")
        if not path:
            return []
        return json.loads(Path(path).expanduser().read_text(encoding="utf-8"))

    @contextlib.contextmanager
    def _key_env(self) -> Iterator[None]:
        if os.environ.get(JEV_KEY_ENV):
            yield
            return
        if self.mode == "fake":
            os.environ[JEV_KEY_ENV] = "offline-fake-jev"
        else:
            if self._key is None:
                self._key = keychain_key(self.service, self.account)
            os.environ[JEV_KEY_ENV] = self._key
        try:
            yield
        finally:
            os.environ.pop(JEV_KEY_ENV, None)

    def load(self, path: str | Path, paths: list[str] | None = None) -> Any:
        """Load a program; the key exists in this process only while the
        runtime child is spawned, which is when the child copies it."""
        binary = find_jevscript()
        with self._key_env():
            return load(str(path), bin=binary, paths=paths)

    def close(self) -> None:
        if self.fake is not None:
            self.fake.stop()


@dataclass
class Episode:
    task: str
    subject: str
    wake_id: str
    result: Any
    recording: str
    usage: dict[str, Any] = field(default_factory=dict)
    verified: bool = False
    problem: str | None = None
    paused: str | None = None  # the decision key a parked run waits on


class Episodes:
    """Runs and resumes episodes; records each in the ledger."""

    def __init__(self, home: Home, session: JevSession, ledger: Any, decisions: Any) -> None:
        self.home = home
        self.session = session
        self.ledger = ledger
        self.decisions = decisions
        self.parked: dict[str, tuple[Any, Any, int]] = {}
        self._seq = 0

    @property
    def paused_dir(self) -> Path:
        return self.home.state / "paused"

    def recording_path(self, task: str, subject: str) -> Path:
        self._seq += 1
        stamp = iso().replace(":", "").replace("-", "")
        safe = "".join(c if c.isalnum() or c in "-_" else "-" for c in subject)[:40]
        # A fresh host (a mate's inline wake, a restart) restarts `_seq`, so a
        # random tag keeps every recording destination new (spec section 10.3).
        return self.home.state / "recordings" / f"{stamp}-{os.getpid()}-{self._seq}-{secrets.token_hex(3)}-{task}-{safe}.jsonl"

    def _begin(self, bind: dict[str, Any], wake_id: str) -> None:
        for adapter in bind.values():
            begin = getattr(adapter, "begin", None)
            if begin is not None:
                begin(wake_id)

    def run(
        self,
        program: Any,
        task: str,
        snapshot: dict[str, Any],
        bind: dict[str, Any],
        *,
        subject: str = "",
        wake_id: str | None = None,
        output: str = "result",
    ) -> Episode:
        wake_id = wake_id or f"w{int(now() * 1000)}-{self._seq}"
        path = self.recording_path(task, subject or task)
        self._begin(bind, wake_id)
        inputs = {"snapshot": snapshot} if output == "result" else snapshot
        run = program.task(task).start(
            inputs=inputs,
            bind=bind,
            record=str(path),
            model=self.session.model,
            profiles=self.session.profiles,
            on_log=lambda line: self.ledger.log(wake=wake_id, subject=subject, recording=str(path), line=line),
        )
        meta = {"task": task, "subject": subject, "wake_id": wake_id, "recording": str(path), "inputs": inputs, "output": output, "started": now()}
        return self._drive(program, run, meta, bind, skip=0, answer=None)

    def _drive(self, program: Any, run: Any, meta: dict[str, Any], bind: dict[str, Any], *, skip: int, answer: dict[str, Any] | None, seen: int = 0) -> Episode:
        """Iterate a run to its end or to a question nobody has answered."""
        result: Any = None
        usage: dict[str, Any] = {}
        verified = False
        problem: str | None = None
        retries = 0
        confirms = seen
        subject = meta["subject"] or meta["task"]
        for pause in run:
            kind = pause.get("kind")
            if kind == "done":
                result = (pause.get("outputs") or {}).get(meta["output"])
                usage = pause.get("usage") or {}
                verified = bool(pause.get("verified"))
            elif kind == "waiting":
                continue
            elif kind == "confirm":
                confirms += 1
                if confirms <= skip:
                    continue  # replaying an earlier, already-answered question
                if answer is not None:
                    run.resume(answer)
                    answer = None
                    continue
                key = f"{subject}:ask"
                self.decisions.record(key, pause.get("message") or "Confirm?", pause.get("options") or ["yes", "no"], {"wake": meta["wake_id"]})
                write_json(self.paused_dir / f"{meta['wake_id']}.json", {**meta, "key": key, "confirms": confirms, "at": iso()})
                self.parked[meta["wake_id"]] = (program, run, confirms)
                return Episode(meta["task"], meta["subject"], meta["wake_id"], None, meta["recording"], paused=key)
            elif kind == "error" and pause.get("retryable") and retries < 2:
                retries += 1
                run.resume({"retry": True})
            else:
                problem = f"{kind}: {pause.get('reason') or pause.get('message') or pause.get('code') or pause.get('key') or ''}".strip()
                if kind in ("escalate", "budget", "error"):
                    self.decisions.record(
                        f"{subject}:{kind}",
                        f"The Chief of Staff stopped while handling {subject}: {problem}",
                        ["retry", "dismiss"],
                        {"recording": meta["recording"]},
                    )
                if kind in ("escalate", "budget") and not getattr(run, "_ended", True):
                    run.abort()
        self.ledger.episode(
            task=meta["task"],
            subject=meta["subject"],
            wake=meta["wake_id"],
            recording=meta["recording"],
            calls=usage.get("calls", 0),
            usd=usage.get("usd", 0.0),
            seconds=round(now() - meta["started"], 3),
            verified=verified,
            problem=problem,
            resumed=meta.get("resumed", False),
        )
        return Episode(meta["task"], meta["subject"], meta["wake_id"], result, meta["recording"], usage, verified, problem)

    # -- parked runs --------------------------------------------------------------

    def parked_records(self) -> list[dict[str, Any]]:
        return [read_json(p, {}) for p in sorted(self.paused_dir.glob("*.json"))]

    def resume(self, wake_id: str, answer: str, program: Any, bind: dict[str, Any]) -> Episode:
        """Answer a parked run's open question and let it finish.

        In memory the run simply continues. After a restart the run is rebuilt
        from its recording: replay serves every earlier event, including the
        earlier answers, and this answer leaves replay and continues live."""
        record = read_json(self.paused_dir / f"{wake_id}.json", None)
        if record is None:
            raise KeyError(f"no parked episode for wake `{wake_id}`")
        payload = {"answer": answer.split(":", 1)[0].strip().lower() if answer.lower().split(":")[0].strip() in ("yes", "no") else answer, "text": answer}
        self._begin(bind, wake_id)
        live = self.parked.pop(wake_id, None)
        (self.paused_dir / f"{wake_id}.json").unlink(missing_ok=True)
        if live is not None:
            _, run, confirms = live
            run.resume(payload)
            return self._drive(program, run, record, bind, skip=0, answer=None, seen=confirms)
        run = program.task(record["task"]).start(
            inputs=record["inputs"],
            bind=bind,
            replay=record["recording"],
            model=self.session.model,
            profiles=self.session.profiles,
            on_log=lambda line: self.ledger.log(
                wake=wake_id, subject=record["subject"], recording=record["recording"], line=line
            ),
        )
        record = {**record, "resumed": True, "started": now()}
        episode = self._drive(program, run, record, bind, skip=record["confirms"] - 1, answer=payload)
        # A replayed run that leaves replay writes no file of its own
        # (spec section 10.1); keep its event stream beside the original.
        events = list(run.events())
        with open(record["recording"] + ".resumed.jsonl", "w", encoding="utf-8") as handle:
            for event in events:
                handle.write(json.dumps(event) + "\n")
        return episode
