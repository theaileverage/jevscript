"""Binding capabilities to external adapter processes.

Agent adapters are not part of the CoS: a person binds whatever
agent they use (Claude Code, Codex, a browser agent, a fake for tests) as an
external process that speaks the Jevscript CLI's language-neutral JSONL
adapter protocol (``crates/jevscript-cli/README.md``). One JSON request per
line on the process's stdin::

    {"operation":"call","capability":"crew","verb":"spawn","args":{"positional":[],"named":{...}}}
    {"operation":"observe","capability":"crew","handle":{...}}

and one reply per line on its stdout: ``{"result": ...}``,
``{"observation": ...}`` or ``{"error": {"message", "retryable"}}``.

``ExternalAdapter`` is the thin host-side binding: it launches the command
once, forwards every capability call from the runtime (and every direct
observation the host's watcher makes) unchanged, and bounds each reply with a
timeout. The SDK cannot abort an in-flight step, so a hung adapter is killed
and reported as a retryable ``adapter_error`` instead of hanging the wake.
"""

from __future__ import annotations

import json
import os
import queue
import shlex
import subprocess
import threading
from pathlib import Path
from typing import Any


class AdapterError(RuntimeError):
    def __init__(self, message: str, retryable: bool = False) -> None:
        super().__init__(message)
        self.retryable = retryable


class ExternalAdapter:
    """A capability bound to a JSONL adapter subprocess (spec section 11.6)."""

    def __init__(
        self,
        command: str | list[str],
        kind: str,
        capability: str,
        *,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        timeout: float = 180.0,
        log: Path | None = None,
        manifest: dict[str, Any] | None = None,
    ) -> None:
        self.command = command if isinstance(command, list) else ["/bin/sh", "-c", command]
        self.kind = kind
        self.capability = capability
        self.cwd = cwd
        self.env = env
        self.timeout = timeout
        self.log = log
        if manifest is not None:
            self.manifest = manifest
        self._process: subprocess.Popen[str] | None = None
        self._replies: queue.Queue[str | None] = queue.Queue()
        self._lock = threading.Lock()

    def bind(self, name: str) -> None:
        """The SDK tells an adapter the name it was bound under."""
        self.capability = name

    # -- process ----------------------------------------------------------------

    def _start(self) -> subprocess.Popen[str]:
        if self._process is not None and self._process.poll() is None:
            return self._process
        stderr = open(self.log, "a", encoding="utf-8") if self.log else subprocess.DEVNULL  # noqa: SIM115
        env = {**os.environ, **(self.env or {})}
        env.pop("TYPESAFE_API_KEY", None)  # an adapter never needs the Jev key
        self._process = subprocess.Popen(  # noqa: S603 - the person configured this command
            self.command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
            text=True,
            bufsize=1,
            cwd=self.cwd,
            env=env,
        )
        self._replies = queue.Queue()
        process = self._process

        def pump() -> None:
            assert process.stdout is not None
            for line in process.stdout:
                if line.strip():
                    self._replies.put(line)
            self._replies.put(None)

        threading.Thread(target=pump, daemon=True).start()
        return self._process

    def _request(self, message: dict[str, Any], timeout: float | None = None) -> dict[str, Any]:
        with self._lock:
            process = self._start()
            assert process.stdin is not None
            try:
                process.stdin.write(json.dumps(message) + "\n")
                process.stdin.flush()
            except BrokenPipeError as error:
                self._process = None
                raise AdapterError(f"the `{self.capability}` adapter exited", retryable=True) from error
            try:
                line = self._replies.get(timeout=timeout or self.timeout)
            except queue.Empty:
                self.close()
                raise AdapterError(
                    f"the `{self.capability}` adapter did not answer within {timeout or self.timeout:.0f}s; it was stopped",
                    retryable=True,
                ) from None
            if line is None:
                self._process = None
                raise AdapterError(f"the `{self.capability}` adapter exited without answering", retryable=True)
            try:
                reply = json.loads(line)
            except json.JSONDecodeError as error:
                raise AdapterError(f"the `{self.capability}` adapter wrote malformed JSON: {line[:200]!r}") from error
        if not isinstance(reply, dict):
            raise AdapterError(f"the `{self.capability}` adapter replied with {type(reply).__name__}")
        error = reply.get("error")
        if error is not None:
            raise AdapterError(str(error.get("message", error)), retryable=bool(error.get("retryable")))
        return reply

    # -- the adapter surface the SDK calls -------------------------------------------

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        timeout = None
        if verb == "wait":
            minutes = (args.get("named") or {}).get("minutes", 5)
            timeout = float(minutes) * 60 + 60
        reply = self._request(
            {"operation": "call", "capability": capability or self.capability, "verb": verb, "args": args},
            timeout,
        )
        if "result" not in reply:
            raise AdapterError(f"the `{self.capability}` adapter's reply to `{verb}` has no result")
        return reply["result"]

    def observe(self, handle: Any, capability: str | None = None) -> dict[str, Any]:
        reply = self._request({"operation": "observe", "capability": capability or self.capability, "handle": handle})
        if "observation" not in reply:
            raise AdapterError(f"the `{self.capability}` adapter's observe reply has no observation")
        return reply["observation"]

    # -- conveniences for the host's own use ------------------------------------------

    def verb(self, verb: str, *positional: Any, **named: Any) -> Any:
        args: dict[str, Any] = {"positional": list(positional)}
        if named:
            args["named"] = named
        return self.call(verb, args)

    def close(self) -> None:
        process, self._process = self._process, None
        if process is None:
            return
        try:
            if process.stdin:
                process.stdin.close()
            process.wait(timeout=5)
        except (subprocess.TimeoutExpired, OSError):
            process.kill()


def from_config(entry: dict[str, Any] | str, kind: str, capability: str, *, log: Path | None = None, cwd: str | None = None) -> ExternalAdapter:
    """``{"command": "...", "timeout": 180, "env": {...}}`` or a bare command."""
    if isinstance(entry, str):
        entry = {"command": entry}
    command = entry["command"]
    if isinstance(command, str) and command.startswith("["):
        command = json.loads(command)
    return ExternalAdapter(
        command if isinstance(command, list) else str(command),
        kind,
        capability,
        cwd=entry.get("cwd", cwd),
        env={k: str(v) for k, v in (entry.get("env") or {}).items()},
        timeout=float(entry.get("timeout", 180)),
        log=log,
    )


def describe(command: list[str]) -> str:
    return " ".join(shlex.quote(c) for c in command)


class AgentBinding:
    """The `crew` capability: an external adapter, with idempotent effects.

    ``spawn``, ``send`` and ``stop`` carry the wake's idempotency keys (see
    ``effects.EffectLog``), so a wake that re-runs after a crash reattaches to
    the worker it already spawned instead of spawning a second one; ``observe``
    and ``wait`` are reads and always reach the adapter.
    """

    kind = "agent"
    EFFECTS = ("spawn", "send", "stop")

    def __init__(self, adapter: Any, effects: Any) -> None:
        self.adapter = adapter
        self.effects = effects
        self.capability = getattr(adapter, "capability", "crew")

    def bind(self, name: str) -> None:
        self.capability = name
        bind = getattr(self.adapter, "bind", None)
        if bind is not None:
            bind(name)

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        if verb in self.EFFECTS:
            return self.effects.once(f"crew.{verb}", args, lambda: self.adapter.call(verb, args, capability))
        return self.adapter.call(verb, args, capability)

    def observe(self, handle: Any, capability: str | None = None) -> dict[str, Any]:
        return self.adapter.observe(handle, capability)

    def verb(self, verb: str, *positional: Any, **named: Any) -> Any:
        args: dict[str, Any] = {"positional": list(positional)}
        if named:
            args["named"] = named
        return self.call(verb, args)

    def close(self) -> None:
        close = getattr(self.adapter, "close", None)
        if close is not None:
            close()
