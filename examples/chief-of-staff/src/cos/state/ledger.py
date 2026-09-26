"""The work ledger: a durable, append-only record of what happened and why.

Two record kinds share ``data/ledger.jsonl``:

- ``episode``: one bounded Jevscript run (which wake, which task, its
  recording, calls, spend and how it ended). Every decision the Chief of
  Staff made is reproducible from the recording it names.
- ``task``: one finished piece of work (the request, how intake routed it,
  its outcome, how long it took, how much supervision it needed, and the
  recordings of every episode that touched it). This is what learning reads.

``log`` lines written by Jevscript are indexed here as ``log`` records too.
The recording remains the source of truth; this index supports progress and
cross-run search without replaying every file.
"""

from __future__ import annotations

from typing import Any

from .home import Home, append_jsonl, iso, now, read_jsonl


def words_minutes(minutes: float) -> str:
    """Numbers become words before Jev sees them (spec section 6.8)."""
    if minutes < 2:
        return "about a minute"
    if minutes < 60:
        return f"about {round(minutes)} minutes"
    hours = minutes / 60
    return "about an hour" if hours < 1.5 else f"about {round(hours)} hours"


def words_count(n: int, noun: str) -> str:
    names = ["no", "one", "two", "three", "four", "five", "six"]
    word = names[n] if n < len(names) else "many"
    return f"{word} {noun}{'' if n == 1 else 's'}"


class Ledger:
    def __init__(self, home: Home) -> None:
        self.home = home
        self.path = home.data / "ledger.jsonl"

    def episode(self, **fields: Any) -> dict[str, Any]:
        record = {"type": "episode", "at": iso(), "ts": now(), **fields}
        append_jsonl(self.path, record)
        return record

    def log(self, *, wake: str, subject: str, recording: str, line: dict[str, Any]) -> dict[str, Any]:
        """Index one live section 5.8 log event, once per callback."""
        return self.record(
            "log",
            wake=wake,
            subject=subject,
            recording=recording,
            level=line.get("level"),
            message=line.get("message"),
            fields=line.get("fields", {}),
            task=line.get("task"),
            source=line.get("source"),
        )

    def task(self, **fields: Any) -> dict[str, Any]:
        record = {"type": "task", "at": iso(), "ts": now(), **fields}
        append_jsonl(self.path, record)
        return record

    def record(self, kind: str, **fields: Any) -> dict[str, Any]:
        record = {"type": kind, "at": iso(), "ts": now(), **fields}
        append_jsonl(self.path, record)
        return record

    def records(self, kind: str | None = None) -> list[dict[str, Any]]:
        rows = read_jsonl(self.path)
        return rows if kind is None else [r for r in rows if r.get("type") == kind]

    def episodes_for(self, subject: str) -> list[dict[str, Any]]:
        return [r for r in self.records("episode") if r.get("subject") == subject]

    def tasks_since(self, ts: float) -> list[dict[str, Any]]:
        return [r for r in self.records("task") if r.get("ts", 0) > ts]

    def learning_entries(self, limit: int = 40) -> list[dict[str, Any]]:
        """Finished tasks shaped for ``learning.learn``: short, in words."""
        rows = []
        for task in self.records("task")[-limit:]:
            rows.append(
                {
                    "id": task["id"],
                    "request": task.get("request", "")[:400],
                    "trigger": task.get("trigger") or task.get("request", "")[:120],
                    "project": task.get("project"),
                    "kind": task.get("kind", "ship"),
                    "effort": task.get("effort", "medium"),
                    "profile": task.get("profile", "default"),
                    "outcome": task.get("outcome", "unknown"),
                    "minutes_words": words_minutes(task.get("minutes", 0)),
                    "nudges_words": words_count(task.get("nudges", 0), "nudge"),
                    "obstacle": task.get("obstacle") or "",
                }
            )
        return rows
