"""The backlog: work items with dependencies, holds and done history.

An item in ``data/backlog.json`` moves ``queued -> in_flight ->
done | failed``; a hold (``{reason, until}``) keeps a queued item from
starting until the person answers, or until a time gate passes. Readiness is
decided by ``routing.ready`` in Jevscript, not here: this module only stores.
"""

from __future__ import annotations

import re
from typing import Any

from .home import Home, append_jsonl, iso, now, read_json, read_jsonl, write_json

STATUSES = ("queued", "in_flight", "done", "failed", "cancelled")


def slug(text: str, limit: int = 24) -> str:
    words = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return (words[:limit].rstrip("-")) or "task"


class Backlog:
    def __init__(self, home: Home, keep_done: int = 20) -> None:
        self.home = home
        self.keep_done = keep_done

    @property
    def path(self):
        return self.home.data / "backlog.json"

    @property
    def history_path(self):
        return self.home.data / "history.jsonl"

    def items(self) -> list[dict[str, Any]]:
        return read_json(self.path, [])

    def get(self, item_id: str) -> dict[str, Any]:
        for item in self.items():
            if item["id"] == item_id:
                return item
        for item in read_jsonl(self.history_path):
            if item["id"] == item_id:
                return item
        raise KeyError(f"no backlog item `{item_id}`")

    def _save(self, items: list[dict[str, Any]]) -> None:
        done = [i for i in items if i["status"] in ("done", "failed", "cancelled")]
        overflow = done[: max(0, len(done) - self.keep_done)]
        for item in overflow:
            append_jsonl(self.history_path, item)
        gone = {i["id"] for i in overflow}
        write_json(self.path, [i for i in items if i["id"] not in gone])

    def next_id(self, title: str) -> str:
        counter = read_json(self.home.state / "counter.json", {"n": 0})
        counter["n"] += 1
        write_json(self.home.state / "counter.json", counter)
        return f"t{counter['n']}-{slug(title)}"

    def add(self, fields: dict[str, Any]) -> dict[str, Any]:
        item = {
            "id": fields.get("id") or self.next_id(fields.get("title") or fields["text"]),
            "title": fields.get("title") or fields["text"][:72],
            "text": fields["text"],
            "project": fields.get("project"),
            "kind": fields.get("kind", "ship"),
            "effort": fields.get("effort", "medium"),
            "profile": fields.get("profile", "default"),
            "notes": fields.get("notes", ""),
            "playbook": fields.get("playbook"),
            "deps": list(fields.get("deps", [])),
            "hold": fields.get("hold"),
            "status": "queued",
            "created": iso(),
            "request": fields.get("request"),
            "signals": fields.get("signals", {}),
        }
        items = self.items()
        if any(i["id"] == item["id"] for i in items):
            raise ValueError(f"backlog already has `{item['id']}`")
        self._save(items + [item])
        return item

    def update(self, item_id: str, **changes: Any) -> dict[str, Any]:
        items = self.items()
        for item in items:
            if item["id"] == item_id:
                item.update(changes)
                self._save(items)
                return item
        raise KeyError(f"no backlog item `{item_id}`")

    def hold(self, item_id: str, reason: str, until: float | None = None) -> dict[str, Any]:
        return self.update(item_id, hold={"reason": reason, "until": until})

    def release(self, item_id: str) -> dict[str, Any]:
        return self.update(item_id, hold=None)

    def start(self, item_id: str) -> dict[str, Any]:
        return self.update(item_id, status="in_flight", started=iso(), started_at=now())

    def finish(self, item_id: str, status: str, outcome: str) -> dict[str, Any]:
        if status not in STATUSES:
            raise ValueError(status)
        return self.update(item_id, status=status, outcome=outcome, finished=iso(), finished_at=now())

    def done_history(self) -> list[dict[str, Any]]:
        done = [i for i in self.items() if i["status"] in ("done", "failed", "cancelled")]
        return read_jsonl(self.history_path) + done

    def snapshot(self) -> list[dict[str, Any]]:
        """What ``routing.ready`` reads: every current item, holds normalized."""
        rows = []
        for item in self.items():
            hold = item.get("hold")
            rows.append(
                {
                    **item,
                    "hold": None if hold is None else {"reason": hold.get("reason", ""), "until": hold.get("until")},
                }
            )
        return rows
