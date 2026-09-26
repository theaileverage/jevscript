"""A reattachable JSONL agent adapter over the terminal backends (9.1).

The terminal belongs to this adapter; the Chief of Staff owns the worktree.
Each handle stores the terminal backend's exact endpoint so a restarted
adapter can observe the same agent without creating another session.
"""

from __future__ import annotations

import json
import os
import shlex
import sys
import time
from pathlib import Path
from typing import Any

from . import backends
from .backends.base import Backend, BackendError


class TmuxAgent:
    """Five agent verbs over any configured terminal backend (section 9.1)."""

    def __init__(self, backend: Backend | None = None) -> None:
        self._fixed_backend = backend
        self._backends: dict[str, Backend] = {}

    def _backend(self, name: str | None = None) -> Backend:
        if self._fixed_backend is not None:
            if name not in (None, "auto", self._fixed_backend.name):
                raise BackendError(f"handle uses {name}, but adapter is bound to {self._fixed_backend.name}", retryable=False)
            return self._fixed_backend
        chosen = name or os.environ.get("COS_BACKEND") or "auto"
        if chosen == "auto":
            chosen = backends.detect()
        if chosen not in self._backends:
            options: dict[str, Any] = {}
            if chosen == "tmux":
                options["session"] = os.environ.get("COS_TMUX_SESSION", "cos")
            self._backends[chosen] = backends.make(chosen, **options)
        return self._backends[chosen]

    @staticmethod
    def launch_argv(named: dict[str, Any]) -> list[str]:
        harness = str(named.get("harness") or "codex")
        command = named.get("command") or os.environ.get(f"COS_AGENT_{harness.upper().replace('-', '_')}_COMMAND") or harness
        argv = shlex.split(command) if isinstance(command, str) else list(command)
        if not argv:
            raise ValueError("an agent launch command is required")
        model, effort = named.get("model"), named.get("effort")
        if harness == "codex":
            if model:
                argv += ["--model", str(model)]
            if effort:
                argv += ["-c", f"model_reasoning_effort={json.dumps(str(effort))}"]
        elif harness == "claude":
            if model:
                argv += ["--model", str(model)]
            if effort:
                argv += ["--effort", str(effort)]
        argv.append(str(named.get("prompt") or ""))
        return argv

    def spawn(self, named: dict[str, Any]) -> dict[str, Any]:
        backend = self._backend(named.get("backend"))
        name = str(named["name"])
        where = named.get("in") or {}
        cwd = str(where.get("path") or where.get("cwd") or named.get("cwd") or "")
        if not cwd or not Path(cwd).is_dir():
            raise BackendError(f"agent worktree does not exist: {cwd}", retryable=False)
        existing = backend.find(name)
        if existing is not None:
            if not backend.alive(existing):
                raise BackendError(f"agent {name} has a dead pane; inspect it before relaunch", retryable=False)
            return self._handle(existing, named)
        argv = self.launch_argv(named)
        ep = backend.spawn(name, cwd)
        try:
            backend.run_line(ep, "exec " + shlex.join(argv))
        except Exception:
            backend.close(ep)
            raise
        return self._handle(ep, named)

    @staticmethod
    def _handle(ep: dict[str, Any], named: dict[str, Any]) -> dict[str, Any]:
        return {"id": str(named["name"]), "endpoint": ep, "harness": named.get("harness") or "codex"}

    def _endpoint(self, handle: dict[str, Any]) -> tuple[Backend, dict[str, Any]]:
        ep = handle.get("endpoint")
        if not isinstance(ep, dict) or not ep.get("backend"):
            raise BackendError("agent handle is missing a terminal endpoint", retryable=False)
        backend = self._backend(ep["backend"])
        if backend.name == "tmux" and ep.get("session") != getattr(backend, "session", None):
            raise BackendError("tmux agent handle belongs to another session", retryable=False)
        return backend, ep

    def observe(self, handle: dict[str, Any]) -> dict[str, Any]:
        backend, ep = self._endpoint(handle)
        if not backend.alive(ep):
            return {"status": "exited", "last_message": "", "tail": "", "exit_code": None, "busy": False}
        tail = backend.read(ep, 120)
        lines = [line.strip() for line in tail.splitlines() if line.strip()]
        harness = str(handle.get("harness") or "")
        pattern = os.environ.get(f"COS_AGENT_{harness.upper().replace('-', '_')}_BUSY", "")
        busy = backend.busy(ep, pattern)
        return {
            "status": "running" if busy else "waiting",
            "last_message": lines[-1] if lines else "",
            "tail": tail,
            "exit_code": None,
            "busy": busy,
        }

    def send(self, handle: dict[str, Any], message: str) -> dict[str, Any]:
        backend, ep = self._endpoint(handle)
        if not backend.alive(ep):
            raise BackendError("tmux agent pane has exited", retryable=False)
        state = backend.submit(ep, message)
        if state == "pending":
            raise BackendError("tmux agent text remains pending in the composer; inspect before retrying", retryable=False)
        return {"state": state}

    def wait(self, handle: dict[str, Any], minutes: float = 0) -> dict[str, Any]:
        deadline = time.monotonic() + max(0.0, min(float(minutes), 2.0)) * 60
        observation = self.observe(handle)
        while observation["status"] == "running" and time.monotonic() < deadline:
            time.sleep(min(1.0, max(0.0, deadline - time.monotonic())))
            observation = self.observe(handle)
        return observation

    def stop(self, handle: dict[str, Any]) -> None:
        backend, ep = self._endpoint(handle)
        if backend.alive(ep) and not backend.close(ep):
            raise BackendError("agent terminal did not close")

    def serve_one(self, request: dict[str, Any]) -> dict[str, Any]:
        operation = request.get("operation")
        if operation == "observe":
            return {"observation": self.observe(request["handle"])}
        if operation != "call":
            raise ValueError(f"unknown adapter operation {operation!r}")
        verb = request.get("verb")
        args = request.get("args") or {}
        positional, named = args.get("positional") or [], args.get("named") or {}
        if verb == "spawn":
            return {"result": self.spawn(named)}
        handle = positional[0] if positional else named.get("handle")
        if not isinstance(handle, dict):
            raise ValueError(f"{verb} requires an agent handle")
        if verb == "observe":
            return {"result": self.observe(handle)}
        if verb == "wait":
            return {"result": self.wait(handle, named.get("minutes", 0))}
        if verb == "send":
            message = positional[1] if len(positional) > 1 else named.get("text", "")
            self.send(handle, str(message))
            return {"result": None}
        if verb == "stop":
            self.stop(handle)
            return {"result": None}
        raise ValueError(f"unknown agent verb {verb!r}")


TerminalAgent = TmuxAgent


def main() -> None:
    agent = TmuxAgent()
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            reply = agent.serve_one(json.loads(line))
        except (BackendError, ValueError, KeyError) as error:
            reply = {"error": {"message": str(error), "retryable": getattr(error, "retryable", False)}}
        except Exception as error:  # noqa: BLE001 - preserve JSONL framing on adapter failure
            reply = {"error": {"message": f"{type(error).__name__}: {error}", "retryable": True}}
        print(json.dumps(reply), flush=True)


if __name__ == "__main__":
    main()
