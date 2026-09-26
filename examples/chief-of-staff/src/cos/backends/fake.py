"""An in-memory backend for tests and offline demos.

Locations are dicts in a process-wide table; typed lines land on the screen
and, when a location has a ``responder``, are handed to it so a test can
script what "runs" there.
"""

from __future__ import annotations

import itertools
from typing import Any, Callable

from .base import Backend, BackendError

_counter = itertools.count(1)


class FakeBackend(Backend):
    name = "fake"
    settle = 0.0
    enter_sleep = 0.0

    #: Every live location, shared by all instances so a reattach finds it.
    panes: dict[str, dict[str, Any]] = {}

    def available(self) -> tuple[bool, str]:
        return True, ""

    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        if self.find(name):
            raise BackendError(f"fake: `{name}` already exists", retryable=False)
        pane_id = f"f{next(_counter)}"
        self.panes[pane_id] = {"name": name, "cwd": cwd, "screen": [], "typed": "", "alive": True, "busy": None, "responder": None}
        return {"backend": self.name, "pane_id": pane_id, "name": name}

    def find(self, name: str) -> dict[str, Any] | None:
        for pane_id, pane in self.panes.items():
            if pane["name"] == name and pane["alive"]:
                return {"backend": self.name, "pane_id": pane_id, "name": name}
        return None

    def pane(self, ep: dict[str, Any]) -> dict[str, Any]:
        return self.panes[ep["pane_id"]]

    def alive(self, ep: dict[str, Any]) -> bool:
        pane = self.panes.get(ep["pane_id"])
        return bool(pane and pane["alive"])

    def read(self, ep: dict[str, Any], lines: int = 200) -> str:
        return "\n".join(self.pane(ep)["screen"][-lines:])

    def native_busy(self, ep: dict[str, Any]) -> bool | None:
        return self.pane(ep)["busy"]

    def type_text(self, ep: dict[str, Any], text: str) -> None:
        self.pane(ep)["typed"] += text

    def key(self, ep: dict[str, Any], key: str) -> None:
        pane = self.pane(ep)
        if key == "Enter":
            line, pane["typed"] = pane["typed"], ""
            pane["screen"].append(f"$ {line}")
            responder: Callable[[dict[str, Any], str], None] | None = pane["responder"]
            if responder is not None:
                responder(pane, line)
        elif key == "C-u":
            pane["typed"] = ""
        else:
            pane["screen"].append(f"<{key}>")

    def close(self, ep: dict[str, Any]) -> bool:
        pane = self.panes.get(ep["pane_id"])
        if pane:
            pane["alive"] = False
        return True
