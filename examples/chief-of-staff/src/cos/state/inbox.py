"""The durable steering inbox, and the status log a worker appends to.

In ``state/<id>.inbox``, an instruction is a sequenced file the
worker reads and acknowledges by moving it into ``handled/``. The worker's
terminal only ever receives a one-line doorbell pointing at the inbox, so
multi-line instructions never depend on how a terminal pastes text, and an
unacknowledged record is re-rung and eventually escalated by supervision.

The status log is ``state/<id>.status``: append-only lines
``<state> [at=<epoch>]: <text>`` written by the worker. A line is a wake
event, not current truth; ``StatusLog.read`` returns what is new since the
last offset the host consumed.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .home import Home, atomic_write, iso, now

STATUS_LINE = re.compile(r"^(?P<kind>working|needs-decision|blocked|paused|done|failed|resolved)\s*(?:\[(?P<meta>[^\]]*)\])?\s*:\s*(?P<text>.*)$")
URL = re.compile(r"https://\S+")


def doorbell(path: Path) -> str:
    return f"Instruction waiting: read each {path}/*.msg in order, act on it, then move it into {path}/handled/."


class Inbox:
    def __init__(self, home: Home, task_id: str) -> None:
        self.dir = home.inbox_dir(task_id)

    def post(self, text: str) -> Path:
        (self.dir / "handled").mkdir(parents=True, exist_ok=True)
        numbers = [int(p.stem) for p in list(self.dir.glob("*.msg")) + list((self.dir / "handled").glob("*.msg")) if p.stem.isdigit()]
        path = self.dir / f"{(max(numbers) if numbers else 0) + 1:03d}.msg"
        atomic_write(path, f"schema=cos-task-inbox.v1\nat={iso()}\n--\n{text.rstrip()}\n")
        return path

    def post_once(self, text: str, key: str) -> Path:
        """A message continuation has one durable worker instruction per ID."""
        marker = f"message_id={key}\n"
        for path in list(self.dir.glob("*.msg")) + list((self.dir / "handled").glob("*.msg")):
            if marker in path.read_text(encoding="utf-8"):
                return path
        return self.post(marker + text)

    def unacked(self) -> list[dict[str, Any]]:
        rows = []
        for path in sorted(self.dir.glob("*.msg")):
            rows.append({"path": str(path), "age_minutes": (now() - path.stat().st_mtime) / 60})
        return rows


@dataclass
class StatusLine:
    kind: str
    text: str
    at: float | None
    raw: str

    @property
    def url(self) -> str | None:
        match = URL.search(self.text)
        return match.group(0).rstrip(".,)") if match else None


def parse_status(line: str) -> StatusLine | None:
    match = STATUS_LINE.match(line.strip())
    if not match:
        return None
    at = None
    meta = match.group("meta") or ""
    stamp = re.search(r"at=(\d+)", meta)
    if stamp:
        at = float(stamp.group(1))
    return StatusLine(match.group("kind"), match.group("text").strip(), at, line.strip())


class StatusLog:
    def __init__(self, home: Home, task_id: str) -> None:
        self.path = home.status_path(task_id)

    def append(self, kind: str, text: str) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(f"{kind} [at={int(now())}]: {text}\n")

    def read(self, offset: int = 0) -> tuple[list[StatusLine], int]:
        """Lines appended since ``offset`` (a byte position) and the new offset."""
        try:
            data = self.path.read_bytes()
        except FileNotFoundError:
            return [], offset
        if offset > len(data):
            offset = 0
        chunk = data[offset:]
        complete = chunk.rfind(b"\n") + 1
        lines = [parse_status(l) for l in chunk[:complete].decode("utf-8", "replace").splitlines()]
        return [l for l in lines if l is not None], offset + complete

    def last(self) -> StatusLine | None:
        lines, _ = self.read(0)
        meaningful = [l for l in lines if l.kind != "resolved"]
        return meaningful[-1] if meaningful else None
