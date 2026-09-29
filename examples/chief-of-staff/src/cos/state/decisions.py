"""Human escalation: decisions, notifications and the digest.

A decision for the principal (the red box) is durable and can be answered
later; nothing blocks while waiting. Each decision is a row in ``state/decisions.json`` keyed
``<subject>:<topic>``; ``cos answer`` closes
it, and the next episode reads the answer from its snapshot. Notifications go
to ``state/outbox.jsonl`` (and, when configured, a desktop notification);
routine news goes to ``state/digest.jsonl`` for the next briefing.

What gets said, and when, is decided in ``escalation.jev``; this module only
stores and delivers.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from typing import Any, Callable

from .home import Home, append_jsonl, iso, now, read_json, read_jsonl, write_json

Notifier = Callable[[str, str], None]


def _applescript(text: str) -> str:
    return text.replace("\\", "\\\\").replace('"', '\\"')


def desktop_notifier(message: str, title: str) -> None:
    """A macOS notification when ``osascript`` exists; silent otherwise."""
    if shutil.which("osascript"):
        script = f'display notification "{_applescript(message[:220])}" with title "{_applescript(title[:80])}"'
        subprocess.run(["osascript", "-e", script], check=False, capture_output=True, timeout=10)


class Decisions:
    def __init__(self, home: Home, notifier: Notifier | None = None, echo: bool = True) -> None:
        self.home = home
        self.path = home.state / "decisions.json"
        self.notifier = notifier
        self.echo = echo

    # -- decisions ------------------------------------------------------------

    def all(self) -> dict[str, dict[str, Any]]:
        return read_json(self.path, {})

    def open(self) -> list[dict[str, Any]]:
        return [d for d in self.all().values() if d.get("answer") is None]

    def record(self, key: str, question: str, options: list[str], context: dict[str, Any] | None = None) -> dict[str, Any]:
        rows = self.all()
        existing = rows.get(key)
        if existing and existing.get("answer") is None:
            return existing  # already asked; asking again would only nag
        rows[key] = {"key": key, "question": question, "options": options, "context": context or {}, "asked": iso(), "answer": None}
        write_json(self.path, rows)
        return rows[key]

    def answer(self, key: str, answer: str) -> dict[str, Any]:
        rows = self.all()
        matches = [k for k in rows if k == key or k.startswith(key)]
        if len(matches) != 1:
            raise KeyError(f"no single open decision matches `{key}`")
        row = rows[matches[0]]
        row["answer"] = answer
        row["answered"] = iso()
        write_json(self.path, rows)
        return row

    def consume(self, key: str) -> dict[str, Any] | None:
        """An answered decision, removed so it is acted on exactly once."""
        rows = self.all()
        row = rows.get(key)
        if row is None or row.get("answer") is None:
            return None
        del rows[key]
        write_json(self.path, rows)
        append_jsonl(self.home.state / "decisions-closed.jsonl", {**row, "closed": iso()})
        return row

    def answered_for(self, subject: str) -> list[dict[str, Any]]:
        return [d for d in self.all().values() if d["key"].startswith(f"{subject}:") and d.get("answer") is not None]

    def pending_for(self, subject: str) -> list[dict[str, Any]]:
        return [d for d in self.all().values() if d["key"].startswith(f"{subject}:") and d.get("answer") is None]

    # -- telling the person -----------------------------------------------------

    def notify(self, message: str) -> None:
        name = self.home.identity["name"]
        append_jsonl(self.home.state / "outbox.jsonl", {"at": iso(), "ts": now(), "name": name, "message": message})
        if self.echo:
            print(f"cos ({name}): {message}", file=sys.stderr)
        if self.notifier is not None:
            self.notifier(message, name)

    def digest(self, message: str) -> None:
        append_jsonl(self.home.state / "digest.jsonl", {"at": iso(), "ts": now(), "message": message})

    def outbox(self, since: float = 0) -> list[dict[str, Any]]:
        return [r for r in read_jsonl(self.home.state / "outbox.jsonl") if r["ts"] > since]

    def drain_digest(self) -> list[dict[str, Any]]:
        path = self.home.state / "digest.jsonl"
        rows = read_jsonl(path)
        if rows:
            append_jsonl(self.home.state / "digest-archive.jsonl", {"at": iso(), "rows": rows})
            path.unlink()
        return rows
