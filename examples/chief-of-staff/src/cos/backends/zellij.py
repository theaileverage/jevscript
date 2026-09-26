"""Zellij: one tab per location in a background session.

This backend needs Zellij 0.44 or newer: only 0.44 can address a pane by id
(``--pane-id``) in a session no
client is attached to. Older releases act on the focused pane, and against a
detached session ``go-to-tab-name`` blocks forever, so ``available`` refuses
them with the reason instead of pretending. ``zellij action`` exits 0 even for
a bad target, so presence is always re-read from ``list-panes``.
"""

from __future__ import annotations

import json
import re
import shutil
from typing import Any

from .base import Backend, BackendError

KEY_NAMES = {"Enter": "Enter", "Escape": "Esc", "C-c": "Ctrl c", "C-u": "Ctrl u"}
MIN_VERSION = (0, 44)


class ZellijBackend(Backend):
    name = "zellij"

    @property
    def session(self) -> str:
        return self.options.get("session") or "cos"

    def z(self, *args: str, check: bool = True) -> str:
        return self.sh(["zellij", "--session", self.session, "action", *args], check=check, timeout=15.0).stdout

    def version(self) -> tuple[int, int]:
        out = self.sh(["zellij", "--version"], check=False).stdout
        match = re.search(r"(\d+)\.(\d+)", out)
        return (int(match.group(1)), int(match.group(2))) if match else (0, 0)

    def available(self) -> tuple[bool, str]:
        if not shutil.which("zellij") and self.options.get("check_path", True):
            return False, "zellij is not installed"
        version = self.version()
        if version < MIN_VERSION:
            return False, f"zellij {version[0]}.{version[1]} cannot address panes in a detached session; 0.44 or newer is needed"
        return True, ""

    def _ensure_session(self) -> None:
        sessions = self.sh(["zellij", "list-sessions", "--short", "--no-formatting"], check=False).stdout.split()
        if self.session not in sessions:
            self.sh(["zellij", "attach", "--create-background", self.session])

    def _panes(self) -> list[dict[str, Any]]:
        try:
            rows = json.loads(self.z("list-panes", "--json", check=False) or "[]")
        except json.JSONDecodeError:
            return []
        return [r for r in rows if not r.get("is_plugin")]

    def _tabs(self) -> list[dict[str, Any]]:
        try:
            return json.loads(self.z("list-tabs", "--json", check=False) or "[]")
        except json.JSONDecodeError:
            return []

    def find(self, name: str) -> dict[str, Any] | None:
        for tab in self._tabs():
            if tab.get("name") == name:
                for pane in self._panes():
                    if pane.get("tab_id") == tab.get("tab_id"):
                        return {"backend": self.name, "session": self.session, "tab_id": tab["tab_id"], "pane_id": pane["id"], "name": name}
        return None

    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        self._ensure_session()
        if self.find(name):
            raise BackendError(f"zellij: a tab named `{name}` already exists", retryable=False)
        active = [t.get("tab_id") for t in self._tabs() if t.get("active")]
        tab_id = int(self.z("new-tab", "--cwd", cwd, "--name", name).strip())
        pane = next((p["id"] for p in self._panes() if p.get("tab_id") == tab_id), None)
        if active:
            self.z("go-to-tab-by-id", str(active[0]), check=False)
        if pane is None:
            raise BackendError("zellij: the new tab has no terminal pane")
        return {"backend": self.name, "session": self.session, "tab_id": tab_id, "pane_id": pane, "name": name}

    def alive(self, ep: dict[str, Any]) -> bool:
        return any(p.get("id") == ep["pane_id"] for p in self._panes())

    def read(self, ep: dict[str, Any], lines: int = 200) -> str:
        args = ["dump-screen", "--pane-id", str(ep["pane_id"])]
        if lines > 40:
            args.append("--full")
        return "\n".join(self.z(*args).rstrip().splitlines()[-lines:])

    def type_text(self, ep: dict[str, Any], text: str) -> None:
        self.z("paste", "--pane-id", str(ep["pane_id"]), "--", text)

    def key(self, ep: dict[str, Any], key: str) -> None:
        self.z("send-keys", "--pane-id", str(ep["pane_id"]), KEY_NAMES[key])

    def close(self, ep: dict[str, Any]) -> bool:
        # close-pane leaves an empty ghost tab; close the whole tab by id.
        self.z("close-tab-by-id", str(ep["tab_id"]), check=False)
        return not self.alive(ep)
