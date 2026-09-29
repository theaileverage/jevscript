"""The JSON-RPC 2.0 client (spec section 11.5).

The runtime is a local process -- ``jevscript serve`` -- and the SDK speaks to
it over stdio, one JSON object per line. Requests go host to runtime; the
runtime asks back for ``capability.call`` and ``capability.observe``, because
adapters live here in the host process and the runtime never links against
tmux, a browser or an agent CLI.
"""

from __future__ import annotations

import json
import subprocess
import threading
from typing import Any, Callable, TextIO

from ._native import resolve_binary

#: Methods the host calls on the runtime.
METHODS = {
    "program_load": "program.load",
    "judgment_run": "judgment.run",
    "task_start": "task.start",
    "run_next": "run.next",
    "run_resume": "run.resume",
    "run_abort": "run.abort",
    "run_inject": "run.inject",
}

#: Requests and notifications the runtime sends back to the host.
HOST_METHODS = {
    "capability_call": "capability.call",
    "capability_observe": "capability.observe",
    "event": "event",
}

HostHandler = Callable[[str, Any], Any]
EventHandler = Callable[[Any], None]


class JevscriptRpcError(Exception):
    """A JSON-RPC error returned by the runtime."""

    def __init__(self, code: int, message: str, data: Any = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"JevscriptRpcError(code={self.code}, message={self.message!r})"


class RpcClient:
    """A line-delimited JSON-RPC client over a spawned ``jevscript serve``.

    The client is synchronous: a run is single-threaded and is stepped by the
    host (spec section 10.5). One reader thread exists only so that a
    ``capability.call`` arriving while the host waits on a reply can be answered.
    """

    def __init__(
        self,
        bin: str | None = None,
        cwd: str | None = None,
        on_host_request: HostHandler | None = None,
        on_event: EventHandler | None = None,
        transport: tuple[TextIO, TextIO] | None = None,
    ) -> None:
        self._on_host_request = on_host_request
        self._on_event = on_event
        self._next_id = 1
        self._lock = threading.Lock()
        self._process = None
        if transport is not None:
            self._input, self._output = transport
        else:
            self._process = subprocess.Popen(
                [resolve_binary(bin), "serve"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                cwd=cwd, text=True, bufsize=1,
            )
            assert self._process.stdout is not None and self._process.stdin is not None
            self._input, self._output = self._process.stdout, self._process.stdin

    def request(self, method: str, params: Any) -> Any:
        """Send a request and wait for its result.

        Raises:
            JevscriptRpcError: if the runtime answers with an error object.
            RuntimeError: if the runtime exits without answering.
        """
        with self._lock:
            request_id = self._next_id
            self._next_id += 1
            self._write({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
            while True:
                message = self._read()
                if message is None:
                    raise RuntimeError("the runtime exited without answering")
                if "method" in message:
                    self._handle_incoming(message)
                    continue
                if message.get("id") != request_id:
                    continue
                error = message.get("error")
                if error is not None:
                    raise JevscriptRpcError(
                        error.get("code", 0), error.get("message", ""), error.get("data")
                    )
                return message.get("result")

    def notify(self, method: str, params: Any) -> None:
        """Send a notification, which takes no reply."""
        with self._lock:
            self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def close(self) -> None:
        """Shut the runtime down."""
        if self._process is None:
            self._output.close()
            self._input.close()
            return
        if self._process.poll() is None:
            try:
                if self._process.stdin is not None:
                    self._process.stdin.close()
                self._process.wait(timeout=5)
            except (subprocess.TimeoutExpired, OSError):  # pragma: no cover - teardown
                self._process.kill()

    def __enter__(self) -> "RpcClient":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    # -- internals ---------------------------------------------------------

    def _write(self, message: Any) -> None:
        self._output.write(json.dumps(message) + "\n")
        self._output.flush()

    def _read(self) -> dict[str, Any] | None:
        while True:
            line = self._input.readline()
            if line == "":
                return None
            line = line.strip()
            if not line:
                continue
            try:
                return json.loads(line)
            except json.JSONDecodeError:  # pragma: no cover - the runtime writes JSON
                continue

    def _handle_incoming(self, message: dict[str, Any]) -> None:
        method = message["method"]
        params = message.get("params")
        if method == HOST_METHODS["event"]:
            if self._on_event is not None:
                self._on_event(params)
            return
        request_id = message.get("id")
        if request_id is None:
            return
        if self._on_host_request is None:
            self._write(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32601, "message": "no adapter is bound for this call"},
                }
            )
            return
        try:
            result = self._on_host_request(method, params)
        except Exception as error:  # noqa: BLE001 - the adapter's error goes back to the runtime
            self._write(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "error": {
                            "message": str(error),
                            "retryable": getattr(error, "retryable", False) is True,
                        }
                    },
                }
            )
            return
        self._write({"jsonrpc": "2.0", "id": request_id, "result": result})
