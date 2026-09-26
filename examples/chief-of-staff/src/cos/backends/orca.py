"""Orca: Orca-managed terminals, in isolated copies Orca creates itself.

Orca owns both the worktree and the terminal, so this is the one backend with
``provides_worktrees``: the
host asks it for the isolated copy instead of running ``git worktree add``.
Terminals are addressed by the runtime-issued handle. Orca has no Escape key
over its CLI; an interrupt is its own ``--interrupt`` input.
"""

from __future__ import annotations

import json
import shutil
from typing import Any

from .base import Backend, BackendError


def _result(text: str) -> dict[str, Any]:
    try:
        body = json.loads(text)
    except json.JSONDecodeError:
        return {}
    if isinstance(body, dict) and body.get("ok") is False:
        raise BackendError(f"orca: {(body.get('error') or {}).get('message', 'request failed')}")
    return (body or {}).get("result", {}) if isinstance(body, dict) else {}


class OrcaBackend(Backend):
    name = "orca"
    provides_worktrees = True

    def o(self, *args: str, check: bool = True) -> dict[str, Any]:
        return _result(self.sh(["orca", *args, "--json"], check=check, timeout=60.0).stdout)

    def available(self) -> tuple[bool, str]:
        if not shutil.which("orca") and self.options.get("check_path", True):
            return False, "orca is not installed"
        try:
            runtime = self.o("status", check=False).get("runtime", {})
        except BackendError as error:
            return False, str(error)
        if not runtime.get("reachable") or runtime.get("state") != "ready":
            return False, "the Orca runtime is not reachable"
        return True, ""

    def create_worktree(self, project_path: str, name: str, base: str) -> dict[str, Any]:
        try:
            self.o("repo", "show", "--repo", f"path:{project_path}")
        except BackendError:
            self.o("repo", "add", "--path", project_path)
        made = self.o("worktree", "create", "--repo", f"path:{project_path}", "--name", name, "--no-parent", "--setup", "skip", "--base-branch", base)
        worktree = made.get("worktree", made)
        path = worktree.get("path")
        if not path:
            raise BackendError("orca: worktree create returned no path")
        return {"path": path, "branch": worktree.get("branch") or name, "base": base, "orca_id": worktree.get("id")}

    def remove_worktree(self, path: str) -> None:
        # Never --force: the host's landed-work check already ran, and Orca's
        # own refusal is a second line of defence for unlanded work.
        self.o("worktree", "rm", "--worktree", f"path:{path}")

    def find(self, name: str) -> dict[str, Any] | None:
        terminals = self.o("terminal", "list", check=False).get("terminals", [])
        for terminal in terminals:
            if terminal.get("title") == f"cos-{name}":
                return {"backend": self.name, "terminal": terminal.get("handle"), "name": name}
        return None

    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        made = self.o("terminal", "create", "--worktree", f"path:{cwd}", "--title", f"cos-{name}")
        handle = (made.get("terminal") or made).get("handle")
        if not handle:
            raise BackendError("orca: terminal create returned no handle")
        return {"backend": self.name, "terminal": handle, "name": name}

    def alive(self, ep: dict[str, Any]) -> bool:
        try:
            self.o("terminal", "read", "--terminal", ep["terminal"], "--limit", "1")
        except BackendError:
            return False
        return True

    def read(self, ep: dict[str, Any], lines: int = 200) -> str:
        body = self.o("terminal", "read", "--terminal", ep["terminal"], "--limit", str(lines))
        terminal = body.get("terminal", body)
        tail = terminal.get("tail") or terminal.get("text") or terminal.get("output") or ""
        return "\n".join(tail) if isinstance(tail, list) else str(tail)

    def type_text(self, ep: dict[str, Any], text: str) -> None:
        self.o("terminal", "send", "--terminal", ep["terminal"], "--text", text)

    def key(self, ep: dict[str, Any], key: str) -> None:
        if key == "Enter":
            self.o("terminal", "send", "--terminal", ep["terminal"], "--text", "", "--enter")
        elif key == "C-c":
            self.o("terminal", "send", "--terminal", ep["terminal"], "--interrupt")
        else:
            raise BackendError(f"orca cannot send {key}", retryable=False)

    def run_line(self, ep: dict[str, Any], line: str) -> None:
        self.o("terminal", "send", "--terminal", ep["terminal"], "--text", line, "--enter")

    def close(self, ep: dict[str, Any]) -> bool:
        self.o("terminal", "close", "--terminal", ep["terminal"], check=False)
        return not self.alive(ep)
