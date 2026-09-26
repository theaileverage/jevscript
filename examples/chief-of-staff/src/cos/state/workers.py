"""Worker records: what the host knows about each dispatched worker.

One JSON file per worker under ``state/workers``. A record is created the
moment an isolated copy exists (before the agent is spawned), so a crash
between the two leaves a record that recovery can finish rather than an
orphaned copy nobody remembers.
"""

from __future__ import annotations

import secrets
from typing import Any

from .home import Home, now, read_json, write_json


class Workers:
    def __init__(self, home: Home) -> None:
        self.home = home

    def get(self, task_id: str) -> dict[str, Any] | None:
        record = read_json(self.home.worker_path(task_id), None)
        return record

    def save(self, record: dict[str, Any]) -> dict[str, Any]:
        write_json(self.home.worker_path(record["id"]), record)
        return record

    def update(self, task_id: str, **changes: Any) -> dict[str, Any]:
        record = self.get(task_id) or {"id": task_id, "worker_id": f"w-{secrets.token_hex(10)}", "created_at": now()}
        if not record.get("worker_id"):
            record["worker_id"] = f"w-{secrets.token_hex(10)}"
        record.update(changes)
        return self.save(record)

    def all(self, live_only: bool = True) -> list[dict[str, Any]]:
        rows = []
        for path in sorted((self.home.state / "workers").glob("*.json")):
            record = read_json(path, None)
            if record and (not live_only or not record.get("retired")):
                rows.append(record)
        return rows

    def retire(self, task_id: str, outcome: str) -> dict[str, Any]:
        return self.update(task_id, retired=True, outcome=outcome, retired_at=now())
