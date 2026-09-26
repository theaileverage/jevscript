"""The durable wake queue.

Every event that needs a decision becomes a wake: a small JSON record in
``state/wakes/``. The episode runner takes wakes in order, runs one bounded
episode for each, persists its outputs, and only then acknowledges the wake by
moving it to ``handled/``. A crash anywhere before that leaves the wake in the
queue, so it runs again (its effects are idempotent by key). A wake whose
episode parked on a question stays in the queue, marked ``paused``, until the
answer resumes it. At most one pending wake exists per kind and subject.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from .state.home import Home, iso, now, read_json, write_json


class WakeQueue:
    def __init__(self, home: Home) -> None:
        self.dir = home.state / "wakes"
        (self.dir / "handled").mkdir(parents=True, exist_ok=True)
        self._counter = home.state / "wake-counter.json"

    def _path(self, wake: dict[str, Any]) -> Path:
        return self.dir / f"{wake['id']}.json"

    def pending(self) -> list[dict[str, Any]]:
        return [read_json(p, {}) for p in sorted(self.dir.glob("*.json"))]

    def find(self, kind: str, subject: str) -> dict[str, Any] | None:
        for wake in self.pending():
            if wake["kind"] == kind and wake["subject"] == subject:
                return wake
        return None

    def push(self, kind: str, subject: str, reasons: list[str] | None = None, **data: Any) -> dict[str, Any]:
        existing = self.find(kind, subject)
        if existing is not None:
            merged = sorted(set(existing.get("reasons", [])) | set(reasons or []))
            if merged != existing.get("reasons"):
                existing["reasons"] = merged
                write_json(self._path(existing), existing)
            return existing
        counter = read_json(self._counter, {"n": 0})
        counter["n"] += 1
        write_json(self._counter, counter)
        wake = {"id": f"{counter['n']:06d}-{kind}-{subject}"[:80], "kind": kind, "subject": subject, "reasons": reasons or [], "at": iso(), "ts": now(), "paused": None, **data}
        write_json(self._path(wake), wake)
        return wake

    def pause(self, wake: dict[str, Any], key: str) -> None:
        wake["paused"] = key
        write_json(self._path(wake), wake)

    def resume(self, wake: dict[str, Any]) -> None:
        wake["paused"] = None
        write_json(self._path(wake), wake)

    def ack(self, wake: dict[str, Any], outcome: str = "handled") -> None:
        path = self._path(wake)
        if path.exists():
            write_json(self.dir / "handled" / path.name, {**wake, "acked": iso(), "outcome": outcome})
            os.unlink(path)
