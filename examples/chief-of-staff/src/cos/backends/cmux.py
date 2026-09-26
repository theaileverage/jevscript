"""cmux: one workspace per location in the cmux macOS app.

Workspaces are created by title without taking focus and addressed by UUID;
the surface to type into is the
first pane's selected surface. Workspace UUIDs do not survive an app relaunch,
so ``find`` matches by title. The app's control socket must be reachable
(``cmux ping`` answers ``PONG``); this backend never launches the app itself.
"""

from __future__ import annotations

import json
import os
import shutil
from typing import Any

from .base import Backend, BackendError

KEY_NAMES = {"Enter": "enter", "Escape": "escape", "C-c": "ctrl-c", "C-u": "ctrl-u"}


class CmuxBackend(Backend):
    name = "cmux"

    def c(self, *args: str, check: bool = True) -> str:
        env = {**os.environ, "CMUX_QUIET": "1"}
        password = self.options.get("socket_password")
        if password:
            env["CMUX_SOCKET_PASSWORD"] = password
        return self.sh(["cmux", *args], check=check, env=env).stdout

    def available(self) -> tuple[bool, str]:
        if not shutil.which("cmux") and self.options.get("check_path", True):
            return False, "cmux is not installed"
        if self.c("ping", check=False).strip() != "PONG":
            return False, "the cmux app is not running or its control socket is not reachable"
        return True, ""

    def _workspace_id(self, title: str) -> str | None:
        try:
            spaces = json.loads(self.c("workspace", "list", "--json", "--id-format", "uuids", check=False) or "{}")
        except json.JSONDecodeError:
            return None
        ids = [w["id"] for w in spaces.get("workspaces", []) if w.get("title") == title]
        return ids[0] if ids else None

    def _surfaces(self, workspace: str) -> list[dict[str, Any]]:
        try:
            return json.loads(self.c("list-panes", "--workspace", workspace, "--json", "--id-format", "uuids", check=False) or "{}").get("panes", [])
        except json.JSONDecodeError:
            return []

    def _endpoint(self, name: str, workspace: str) -> dict[str, Any]:
        panes = self._surfaces(workspace)
        if not panes:
            raise BackendError(f"cmux: workspace `{name}` has no pane")
        surface = panes[0].get("selected_surface_id") or (panes[0].get("surface_ids") or [None])[0]
        return {"backend": self.name, "workspace": workspace, "surface": surface, "name": name}

    def find(self, name: str) -> dict[str, Any] | None:
        workspace = self._workspace_id(f"cos-{name}")
        return self._endpoint(name, workspace) if workspace else None

    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        title = f"cos-{name}"
        if self._workspace_id(title):
            raise BackendError(f"cmux: a workspace titled `{title}` already exists", retryable=False)
        self.c("new-workspace", "--name", title, "--cwd", cwd, "--focus", "false", "--id-format", "uuids")
        workspace = self._workspace_id(title)
        if not workspace:
            raise BackendError(f"cmux: created `{title}` but cannot find it")
        return self._endpoint(name, workspace)

    def alive(self, ep: dict[str, Any]) -> bool:
        return any(ep["surface"] in (p.get("surface_ids") or []) for p in self._surfaces(ep["workspace"]))

    def read(self, ep: dict[str, Any], lines: int = 200) -> str:
        out = self.c("read-screen", "--workspace", ep["workspace"], "--surface", ep["surface"], "--scrollback", "--lines", str(max(lines, 200)), "--json")
        try:
            text = json.loads(out).get("text", "")
        except json.JSONDecodeError:
            text = out
        return "\n".join(text.rstrip().splitlines()[-lines:])

    def type_text(self, ep: dict[str, Any], text: str) -> None:
        self.c("send", "--workspace", ep["workspace"], "--surface", ep["surface"], "--", text)

    def key(self, ep: dict[str, Any], key: str) -> None:
        self.c("send-key", "--workspace", ep["workspace"], "--surface", ep["surface"], KEY_NAMES[key])

    def close(self, ep: dict[str, Any]) -> bool:
        self.c("close-workspace", "--workspace", ep["workspace"], check=False)
        return not self.alive(ep)
