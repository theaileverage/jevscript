"""Herdr: the default backend. One tab per worker in a Chief of Staff workspace.

Workers live as tabs labelled ``cos-<task>`` in one workspace: by default it
finds the workspace labelled
``cos`` (``workspace_label``); with ``workspace: current`` it uses the caller's
own workspace. A labelled
workspace is found by label, creating it once and pruning the
seed tab a fresh workspace comes with. Herdr reports the agent's lifecycle
natively (``agent get``: working is busy; idle, done and blocked are not), so
busy does not depend on screen scraping. Every control command answers JSON;
presence is read from the body (``pane_not_found``), never the exit code.
"""

from __future__ import annotations

import json
import os
import shutil
from typing import Any

from .base import Backend, BackendError

KEY_NAMES = {"Enter": "enter", "Escape": "escape", "C-c": "ctrl+c", "C-u": "ctrl+u"}


def _json(text: str) -> dict[str, Any]:
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        return {}
    return value if isinstance(value, dict) else {}


class HerdrBackend(Backend):
    name = "herdr"

    @property
    def label(self) -> str:
        return self.options.get("workspace_label") or "cos"

    def herdr(self, *args: str, check: bool = True) -> dict[str, Any]:
        argv = ["herdr", *args]
        session = self.options.get("session")
        env = None
        if session:
            argv += ["--session", session]
            env = {**os.environ, "HERDR_SESSION": session}
        result = self.sh(argv, check=False, env=env)
        body = _json(result.stdout)
        if check and (result.returncode != 0 or body.get("error")):
            error = body.get("error") or {}
            raise BackendError(f"herdr {' '.join(args[:2])}: {error.get('message') or result.stderr.strip() or result.returncode}")
        return body

    def available(self) -> tuple[bool, str]:
        if not shutil.which("herdr") and self.options.get("check_path", True):
            return False, "herdr is not installed"
        status = self.herdr("status", "--json", check=False)
        server = status.get("server") or status.get("result", {}).get("server") or {}
        if server and server.get("running") is False:
            return False, "no herdr server is running for this session"
        return True, ""

    # -- workspace ------------------------------------------------------------

    def workspace(self, cwd: str) -> str:
        mode = self.options.get("workspace", "label")
        if mode == "current" and os.environ.get("HERDR_WORKSPACE_ID") and not self.options.get("session"):
            return os.environ["HERDR_WORKSPACE_ID"]
        spaces = self.herdr("workspace", "list").get("result", {}).get("workspaces", [])
        matches = [w["workspace_id"] for w in spaces if w.get("label") == self.label]
        if len(matches) > 1:
            raise BackendError(f"herdr: {len(matches)} workspaces are labelled `{self.label}`; refusing to guess", retryable=False)
        if matches:
            return matches[0]
        created = self.herdr("workspace", "create", "--cwd", cwd, "--label", self.label, "--no-focus").get("result", {})
        workspace_id = created["workspace"]["workspace_id"]
        seed = (created.get("root_pane") or {}).get("pane_id")
        self._seed = (workspace_id, seed)
        return workspace_id

    def _prune_seed(self, workspace_id: str) -> None:
        seed = getattr(self, "_seed", None)
        if not seed or seed[0] != workspace_id or not seed[1]:
            return
        tabs = self.herdr("tab", "list", "--workspace", workspace_id, check=False).get("result", {}).get("tabs", [])
        if len(tabs) > 1:
            self.herdr("pane", "close", seed[1], check=False)
        self._seed = None

    # -- the interface --------------------------------------------------------

    def _tab_label(self, name: str) -> str:
        return f"cos-{name}"

    def find(self, name: str) -> dict[str, Any] | None:
        label = self._tab_label(name)
        spaces = self.herdr("workspace", "list", check=False).get("result", {}).get("workspaces", [])
        for space in spaces:
            wid = space.get("workspace_id")
            tabs = self.herdr("tab", "list", "--workspace", wid, check=False).get("result", {}).get("tabs", [])
            for tab in tabs:
                if tab.get("label") != label:
                    continue
                panes = self.herdr("pane", "list", "--workspace", wid, check=False).get("result", {}).get("panes", [])
                for pane in panes:
                    if pane.get("tab_id") == tab.get("tab_id"):
                        return {"backend": self.name, "workspace_id": wid, "tab_id": tab["tab_id"], "pane_id": pane["pane_id"], "label": label}
        return None

    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        workspace_id = self.workspace(cwd)
        label = self._tab_label(name)
        created = self.herdr("tab", "create", "--workspace", workspace_id, "--cwd", cwd, "--label", label, "--no-focus").get("result", {})
        self._prune_seed(workspace_id)
        return {
            "backend": self.name,
            "workspace_id": workspace_id,
            "tab_id": created["tab"]["tab_id"],
            "pane_id": created["root_pane"]["pane_id"],
            "label": label,
        }

    def alive(self, ep: dict[str, Any]) -> bool:
        body = self.herdr("pane", "get", ep["pane_id"], check=False)
        error = (body.get("error") or {}).get("code")
        return bool(body.get("result")) and error != "pane_not_found"

    def read(self, ep: dict[str, Any], lines: int = 200) -> str:
        # Small --lines values come back empty, so always fetch 200 and trim.
        result = self.sh(["herdr", "pane", "read", ep["pane_id"], "--source", "recent-unwrapped", "--lines", str(max(lines, 200))])
        body = _json(result.stdout)
        text = result.stdout if not body else (body.get("result", {}).get("text") or body.get("result", {}).get("content") or "")
        return "\n".join(text.rstrip().splitlines()[-lines:])

    def native_busy(self, ep: dict[str, Any]) -> bool | None:
        body = self.herdr("agent", "get", ep["pane_id"], check=False)
        status = ((body.get("result") or {}).get("agent") or {}).get("agent_status")
        if status == "working":
            return True
        if status in ("idle", "done", "blocked"):
            return False
        return None

    def type_text(self, ep: dict[str, Any], text: str) -> None:
        self.herdr("pane", "send-text", ep["pane_id"], text)

    def key(self, ep: dict[str, Any], key: str) -> None:
        self.herdr("pane", "send-keys", ep["pane_id"], KEY_NAMES[key])

    def run_line(self, ep: dict[str, Any], line: str) -> None:
        self.herdr("pane", "run", ep["pane_id"], line)

    def close(self, ep: dict[str, Any]) -> bool:
        self.herdr("pane", "close", ep["pane_id"], check=False)
        return not self.alive(ep)
