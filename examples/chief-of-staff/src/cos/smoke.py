"""A live smoke check of one terminal backend on this machine.

Spawns a location in a scratch directory, runs a marker command in it, reads
the marker back, finds the location again by name, types a line, closes it
and confirms it is gone. Skips, with the reason, when the backend is not
installed or not usable here. Two backends need an explicit opt-in because
the check changes state in an application the person is using:

- Herdr: ``COS_SMOKE_HERDR=1`` (it creates and closes a tab in a live Herdr
  server; run it only with explicit lifecycle authorization);
- Orca: ``COS_SMOKE_ORCA_WORKTREE=<path of an Orca-managed worktree>`` (Orca
  terminals live in Orca-managed worktrees, and registering a new repository
  with Orca cannot be undone from its CLI).
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import time
from typing import Any

from . import backends


def smoke(name: str, timeout: float = 15.0) -> dict[str, Any]:
    report: dict[str, Any] = {"backend": name, "result": "skipped", "reason": "", "steps": []}
    options: dict[str, Any] = {}
    cwd = tempfile.mkdtemp(prefix="cos-smoke-")
    if name == "herdr":
        if os.environ.get("COS_SMOKE_HERDR") != "1":
            report["reason"] = "set COS_SMOKE_HERDR=1 to let the check create and close a tab in the running Herdr server"
            return report
        options["workspace_label"] = "cos-smoke"
    if name == "orca":
        worktree = os.environ.get("COS_SMOKE_ORCA_WORKTREE")
        if not worktree:
            report["reason"] = "set COS_SMOKE_ORCA_WORKTREE to an Orca-managed worktree path to run the Orca check"
            return report
        cwd = worktree
    session = f"cos-smoke-{os.getpid()}"
    if name in ("tmux", "zellij"):
        options["session"] = session
    backend = backends.make(name, **options)
    ok, why = backend.available()
    if not ok:
        report["reason"] = why
        return report
    label = f"smoke-{os.getpid()}"
    marker = f"cos-smoke-{os.getpid()}-ok"
    ep = None
    try:
        ep = backend.spawn(label, cwd)
        report["steps"].append({"spawn": ep})
        backend.run_line(ep, f"printf '%s\\n' {marker}")
        deadline = time.time() + timeout
        screen = ""
        while time.time() < deadline:
            screen = backends.strip_ansi(backend.read(ep, 60))
            if any(line.strip() == marker for line in screen.splitlines()):
                break
            time.sleep(0.5)
        else:
            raise AssertionError(f"the marker never appeared; last screen: {screen[-300:]!r}")
        report["steps"].append({"read": "marker seen"})
        assert backend.alive(ep), "the location is not alive"
        found = backend.find(label)
        assert found is not None, "find() did not recover the location by name"
        report["steps"].append({"find": found})
        state = backend.submit(ep, f"printf '%s\\n' {marker}-2")
        report["steps"].append({"submit": state})
        report["steps"].append({"busy": backend.busy(ep, r"")})
        assert backend.close(ep), "close() did not remove the location"
        ep = None
        report["result"] = "passed"
    except Exception as error:  # noqa: BLE001 - the report carries the failure
        report["result"] = "failed"
        report["reason"] = f"{type(error).__name__}: {error}"
    finally:
        if ep is not None:
            try:
                backend.close(ep)
            except Exception:  # noqa: BLE001
                pass
        if name == "tmux":
            subprocess.run(["tmux", "kill-session", "-t", f"={session}"], capture_output=True, check=False)
        if name == "zellij":
            subprocess.run(["zellij", "kill-session", session], capture_output=True, check=False)
            subprocess.run(["zellij", "delete-session", session], capture_output=True, check=False)
    return report
