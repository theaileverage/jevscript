"""tmux: one window per worker in a dedicated session.

Windows are created detached with a fixed name (automatic rename off), then
addressed by their
stable ``#{pane_id}``, read with ``capture-pane``, typed into literally with
``send-keys -l``, and killed by window id. tmux has no native busy state, so
busy comes from the harness footer. tmux falls back to the active window when
a target name is missing, which is why every read targets a pane id and
``alive`` asks for that exact id.
"""

from __future__ import annotations

import shutil
from typing import Any

from .base import Backend, BackendError

KEY_NAMES = {"Enter": "Enter", "Escape": "Escape", "C-c": "C-c", "C-u": "C-u"}


class TmuxBackend(Backend):
    name = "tmux"

    @property
    def session(self) -> str:
        return self.options.get("session") or "cos"

    def available(self) -> tuple[bool, str]:
        if not shutil.which("tmux") and self.options.get("check_path", True):
            return False, "tmux is not installed"
        return True, ""

    def _ensure_session(self, cwd: str) -> None:
        if self.sh(["tmux", "has-session", "-t", f"={self.session}"], check=False).returncode != 0:
            self.sh(["tmux", "new-session", "-d", "-s", self.session, "-c", cwd])

    def _windows(self) -> list[dict[str, str]]:
        result = self.sh(
            ["tmux", "list-windows", "-t", f"={self.session}", "-F", "#{window_name}\t#{window_id}\t#{pane_id}"],
            check=False,
        )
        if result.returncode != 0:
            return []
        rows = []
        for line in result.stdout.splitlines():
            parts = line.split("\t")
            if len(parts) == 3:
                rows.append({"window": parts[0], "window_id": parts[1], "pane_id": parts[2]})
        return rows

    def find(self, name: str) -> dict[str, Any] | None:
        for row in self._windows():
            if row["window"] == name:
                return {"backend": self.name, "session": self.session, **row}
        return None

    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        self._ensure_session(cwd)
        if self.find(name):
            raise BackendError(f"tmux: a window named `{name}` already exists in `{self.session}`", retryable=False)
        out = self.sh(
            ["tmux", "new-window", "-dP", "-F", "#{window_id}\t#{pane_id}", "-t", f"{self.session}:", "-n", name, "-c", cwd]
        ).stdout.strip()
        window_id, pane_id = out.split("\t")
        for option in ("automatic-rename", "allow-rename"):
            self.sh(["tmux", "set-window-option", "-t", window_id, option, "off"], check=False)
        return {"backend": self.name, "session": self.session, "window": name, "window_id": window_id, "pane_id": pane_id}

    def alive(self, ep: dict[str, Any]) -> bool:
        # tmux can resolve a stale target to the active pane of another
        # window. Require the exact window/pane pair in this session first.
        if not any(
            row["window_id"] == ep.get("window_id") and row["pane_id"] == ep.get("pane_id")
            for row in self._windows()
        ):
            return False
        result = self.sh(["tmux", "display-message", "-p", "-t", ep["pane_id"], "#{pane_id} #{pane_dead}"], check=False)
        if result.returncode != 0:
            return False
        parts = result.stdout.split()
        return bool(parts) and parts[0] == ep["pane_id"] and (len(parts) < 2 or parts[1] != "1")

    def read(self, ep: dict[str, Any], lines: int = 200) -> str:
        result = self.sh(["tmux", "capture-pane", "-p", "-J", "-t", ep["pane_id"], "-S", f"-{lines}"])
        return result.stdout.rstrip()

    def type_text(self, ep: dict[str, Any], text: str) -> None:
        self.sh(["tmux", "send-keys", "-t", ep["pane_id"], "-l", text])

    def key(self, ep: dict[str, Any], key: str) -> None:
        self.sh(["tmux", "send-keys", "-t", ep["pane_id"], KEY_NAMES[key]])

    def close(self, ep: dict[str, Any]) -> bool:
        self.sh(["tmux", "kill-window", "-t", ep["window_id"]], check=False)
        return not any(row["window_id"] == ep["window_id"] for row in self._windows())
