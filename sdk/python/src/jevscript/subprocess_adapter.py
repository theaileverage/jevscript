"""Persistent JSONL agent adapter for Python hosts (spec sections 9.5, 11.6).

The executable is the same one accepted by ``jevscript run --bind``. Requests
and replies use the protocol in ``crates/jevscript-cli/README.md``. An argv
sequence launches directly, without a shell.
"""

from __future__ import annotations

import json
import subprocess
import threading
from typing import Any, Sequence


class SubprocessAdapterError(Exception):
    """A spec 11.6 adapter reply error with the protocol's retry hint."""

    def __init__(self, message: str, retryable: bool = False) -> None:
        super().__init__(message)
        self.retryable = retryable


class SubprocessAgentAdapter:
    """A spec 9.1 host binding for one persistent JSONL adapter process.

    Close the process when the host is done. Closing leaves agent panes alone;
    their JSON handles remain reattachable by a new adapter process.
    """

    kind = "agent"

    def __init__(
        self,
        command: str,
        args: Sequence[str] = (),
        *,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
    ) -> None:
        self.capability: str | None = None
        self._lock = threading.Lock()
        self._closed = False
        self._process = subprocess.Popen(
            [command, *args],
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,
            text=True,
            encoding="utf-8",
        )

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        """Forward a spec 9.1 verb and return the adapter's ``result``."""
        reply = self._request(
            {"operation": "call", "capability": capability or self.capability, "verb": verb, "args": args}
        )
        if "result" not in reply:
            raise SubprocessAdapterError("adapter reply has no result")
        return reply["result"]

    def observe(self, handle: dict[str, Any], capability: str | None = None) -> dict[str, Any]:
        """Forward the observation request for a reattachable agent handle."""
        reply = self._request(
            {"operation": "observe", "capability": capability or self.capability, "handle": handle}
        )
        observation = reply.get("observation")
        if not isinstance(observation, dict):
            raise SubprocessAdapterError("adapter reply has no observation")
        return observation

    def close(self) -> None:
        """Close stdin and reap the adapter process."""
        with self._lock:
            if self._closed:
                return
            self._closed = True
            if self._process.stdin is not None:
                try:
                    self._process.stdin.close()
                except BrokenPipeError:
                    pass
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()
            if self._process.stdout is not None:
                self._process.stdout.close()

    def __enter__(self) -> "SubprocessAgentAdapter":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def _request(self, request: dict[str, Any]) -> dict[str, Any]:
        with self._lock:
            if self._closed:
                raise SubprocessAdapterError("adapter connection is closed")
            assert self._process.stdin is not None
            assert self._process.stdout is not None
            try:
                self._process.stdin.write(json.dumps(request) + "\n")
                self._process.stdin.flush()
                line = self._process.stdout.readline()
            except (BrokenPipeError, OSError) as error:
                raise SubprocessAdapterError(f"adapter process failed: {error}") from error
            if line == "":
                raise SubprocessAdapterError("adapter process exited before replying")
            try:
                reply = json.loads(line)
            except json.JSONDecodeError as error:
                raise SubprocessAdapterError("adapter emitted malformed JSON") from error
            if not isinstance(reply, dict):
                raise SubprocessAdapterError("adapter reply must be a JSON object")
            if "error" in reply:
                detail = reply["error"]
                if not isinstance(detail, dict):
                    raise SubprocessAdapterError("adapter error must be an object")
                raise SubprocessAdapterError(
                    str(detail.get("message", "adapter error")), detail.get("retryable") is True
                )
            return reply


def subprocess_agent(
    command: str,
    args: Sequence[str] = (),
    *,
    cwd: str | None = None,
    env: dict[str, str] | None = None,
) -> SubprocessAgentAdapter:
    """Build a subprocess backed ``agent`` binding from an executable and argv."""
    return SubprocessAgentAdapter(command, args, cwd=cwd, env=env)
