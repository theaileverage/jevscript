"""The Python host SDK for Jevscript.

The surface is spec section 11.2, named idiomatically for Python, with a
synchronous iterator of pauses:

```python
program = load("examples/fix_issue.jev")
run = program.task("main").start(inputs={"issue": issue}, bind={"claude": claude})
for pause in run:
    if pause["kind"] == "confirm":
        run.resume({"answer": "yes"})
```

Every call goes to the Rust runtime over JSON-RPC on stdio (spec section 11.5).
Adapters stay here in the host process: the runtime asks back for
``capability.call`` and ``capability.observe``.

The client, runtime methods, adapter callbacks and event stream are all live.
Replays are served entirely from the recording and never call adapters or the
model endpoint.
"""

from __future__ import annotations

import threading
from typing import Any, Callable, Iterator

from .rpc import HOST_METHODS, METHODS, JevscriptRpcError, RpcClient
from .types import (
    TERMINAL_PAUSE_KINDS,
    Adapter,
    LogEvent,
    LogHandler,
    Pause,
    RecordingEvent,
    Resume,
    Sample,
)


def load(
    path_or_source: str,
    *,
    source: bool = False,
    bin: str | None = None,
    cwd: str | None = None,
    paths: list[str] | None = None,
) -> "Program":
    """Load a program by path, or by source when ``source`` is true.

    Spawns one runtime process per program. Close it with :meth:`Program.close`.

    Raises:
        JevscriptRpcError: if the runtime rejects the program.
    """
    bindings: dict[str, dict[str, Adapter]] = {}
    events = _EventHub()

    def on_host_request(method: str, params: Any) -> Any:
        return _dispatch(bindings, method, params)

    def on_event(params: Any) -> None:
        event = (params or {}).get("event")
        if event is not None:
            events.add(event)

    client = RpcClient(bin=bin, cwd=cwd, on_host_request=on_host_request, on_event=on_event)
    if source:
        params: dict[str, Any] = {"source": path_or_source, "paths": paths or []}
    else:
        params = {"path": path_or_source, "paths": paths or []}
    try:
        loaded = client.request(METHODS["program_load"], params)
    except BaseException:
        client.close()
        raise
    return Program(client, loaded, bindings, events)


def log_event(event: RecordingEvent) -> LogEvent | None:
    """The ``log`` line a recording event carries, or ``None`` for any other
    event (spec section 5.8)."""
    if event.get("event") != "log":
        return None
    line: LogEvent = {
        "level": event.get("level"),
        "message": event.get("message"),
        "fields": event.get("fields", {}),
        "task": event.get("task"),
        "source": event.get("source"),
    }
    for key in ("run_id", "seq"):
        if key in event:
            line[key] = event[key]
    return line


class _EventHub:
    """Retain and wake consumers for spec section 11.2's event stream."""

    def __init__(self) -> None:
        self.history: list[RecordingEvent] = []
        self.condition = threading.Condition()
        self.subscribers: list[Callable[[RecordingEvent], None]] = []

    def add(self, event: RecordingEvent) -> None:
        with self.condition:
            self.history.append(event)
            subscribers = list(self.subscribers)
            self.condition.notify_all()
        for subscriber in subscribers:
            subscriber(event)

    def subscribe(self, subscriber: Callable[[RecordingEvent], None], since: int) -> None:
        """Deliver the events held from position ``since``, then every new one."""
        with self.condition:
            held = self.history[since:]
            self.subscribers.append(subscriber)
        for event in held:
            subscriber(event)

    def unsubscribe(self, subscriber: Callable[[RecordingEvent], None]) -> None:
        with self.condition:
            if subscriber in self.subscribers:
                self.subscribers.remove(subscriber)

    def wake(self) -> None:
        with self.condition:
            self.condition.notify_all()


def _dispatch(bindings: dict[str, dict[str, Adapter]], method: str, params: Any) -> Any:
    """Route a ``capability.call`` or ``capability.observe`` to the bound adapter."""
    params = params or {}
    name = params.get("capability", "")
    adapter = bindings.get(params.get("run_id", ""), {}).get(name)
    if adapter is None:
        raise LookupError(f"no adapter is bound for `{name}`")
    if method == HOST_METHODS["capability_observe"]:
        observe = getattr(adapter, "observe", None)
        if observe is None:
            raise LookupError(f"`{name}` cannot be observed")
        return {"observation": observe(params.get("handle"), name)}
    if method == HOST_METHODS["capability_call"]:
        verb = params.get("verb", "")
        result = adapter.call(verb, params.get("args", {}), name)
        return {"result": _encode_adapter_result(adapter, verb, name, result)}
    raise LookupError(f"unknown method `{method}`")


def _encode_adapter_result(adapter: Adapter, verb: str, capability: str, result: Any) -> Any:
    """Encode only results whose declared capability contract makes them handles."""
    manifest = getattr(adapter, "manifest", None)
    signature = (manifest or {}).get("verbs", {}).get(verb, {})
    if signature.get("returns") == "handle":
        if not isinstance(result, dict) or not isinstance(result.get("id"), str):
            raise TypeError(
                f"`{capability}.{verb}` declared a handle result without a string id"
            )
        return {**result, "$jev": "handle", "capability": capability}
    if (
        adapter.kind == "agent"
        and verb == "spawn"
        and isinstance(result, dict)
        and isinstance(result.get("id"), str)
    ):
        return {**result, "capability": capability}
    return result


def _binding(name: str, adapter: Adapter) -> dict[str, Any]:
    """One entry of ``task.start``'s ``bindings``: name, kind and, for a tool,
    its manifest. The adapter itself never leaves the host process."""
    binding: dict[str, Any] = {"name": name, "kind": adapter.kind}
    manifest = getattr(adapter, "manifest", None)
    if manifest is not None:
        binding["manifest"] = manifest
    return binding


class Program:
    """A loaded program."""

    def __init__(
        self,
        client: RpcClient,
        loaded: dict[str, Any],
        bindings: dict[str, dict[str, Adapter]],
        events: _EventHub,
    ) -> None:
        self._client = client
        self._id = loaded["program_id"]
        self._bindings = bindings
        self._events = events
        #: The program's name.
        self.name: str = loaded.get("name", "")
        #: Declared inputs (spec section 3.2).
        self.inputs: list[dict[str, Any]] = loaded.get("inputs", [])
        #: Declared capabilities, with kinds (spec section 3.4).
        self.needs: list[dict[str, Any]] = loaded.get("needs", [])
        #: Judgments with their answer spaces and shape hashes (spec 11.4).
        self.judgments: list[dict[str, Any]] = loaded.get("judgments", [])

    def judgment(self, name: str) -> "Judgment":
        """One judgment, which runs on its own with no bindings (spec 11.3)."""
        return Judgment(self._client, self._id, name)

    def task(self, name: str) -> "Task":
        """One task."""
        return Task(self._client, self._id, name, self._bindings, self._events)

    def close(self) -> None:
        """Shut the runtime process down."""
        self._client.close()

    def __enter__(self) -> "Program":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


class Judgment:
    """A named judgment (spec section 6.7)."""

    def __init__(self, client: RpcClient, program_id: str, name: str) -> None:
        self._client = client
        self._program_id = program_id
        self.name = name

    def run(
        self,
        state: dict[str, Any],
        *,
        model: str | None = None,
        profiles: str | None = None,
        on_log: LogHandler | None = None,
    ) -> Any:
        """Run this judgment against a state: exactly one Jev request, no
        bindings needed (spec section 11.3).

        A judgment run alone has no recording, so ``on_log`` receives the lines
        its ``log`` statements wrote, in order, before this returns (5.8).

        Raises:
            JevscriptRpcError: if the runtime rejects the request.
        """
        params: dict[str, Any] = {
            "program_id": self._program_id,
            "name": self.name,
            "state": state,
        }
        if model is not None:
            params["model"] = model
        if profiles is not None:
            params["profiles"] = profiles
        result = self._client.request(METHODS["judgment_run"], params)
        if on_log is not None:
            for line in result.get("logs", []):
                on_log({**line, "fields": line.get("fields", {})})
        return result.get("answers")


class Task:
    """A task (spec section 7)."""

    def __init__(
        self,
        client: RpcClient,
        program_id: str,
        name: str,
        bindings: dict[str, dict[str, Adapter]],
        events: _EventHub,
    ) -> None:
        self._client = client
        self._program_id = program_id
        self.name = name
        self._bindings = bindings
        self._events = events

    def start(
        self,
        *,
        inputs: dict[str, Any] | None = None,
        bind: dict[str, Adapter] | None = None,
        record: str | None = None,
        replay: str | None = None,
        redact: bool | None = None,
        model: str | None = None,
        sample: Sample | None = None,
        profiles: str | None = None,
        on_log: LogHandler | None = None,
    ) -> "Run":
        """Start the task.

        The returned run is an iterator of pauses: iterate it, answer each pause
        with :meth:`Run.resume`, and the iteration ends at a terminal pause.

        A ``tool`` adapter carrying a ``manifest`` attribute has it checked
        before the run starts (spec section 9.4). ``sample`` draws labels and
        levels from Jev's distribution instead of taking the argmax (6.11),
        ``profiles`` layers a profiles file over the bundled ones (10.6).
        Module ``paths`` belong to :func:`load`, because linking is complete
        before a task starts (3.9). ``record`` and ``replay`` are mutually
        exclusive, and a recording destination must not already exist (10.3).

        ``on_log`` receives each ``log`` line this run writes, as it is written
        (5.8), on the SDK's reader thread; route it anywhere. A replay checks
        the recorded lines but does not emit them again, so a replayed run
        calls ``on_log`` only for lines written after it leaves the recording.

        Raises:
            JevscriptRpcError: if the runtime refuses to start the task, for
                example with ``binding_missing``.
        """
        bind = bind or {}
        for name, adapter in bind.items():
            setattr(adapter, "capability", name)
            bind_hook = getattr(adapter, "bind", None)
            if bind_hook is not None:
                bind_hook(name)
        params: dict[str, Any] = {
            "program_id": self._program_id,
            "name": self.name,
            "inputs": inputs or {},
            "bindings": [_binding(name, adapter) for name, adapter in bind.items()],
        }
        for key, value in (
            ("record", record),
            ("replay", replay),
            ("redact", redact),
            ("model", model),
            ("sample", sample),
            ("profiles", profiles),
        ):
            if value is not None:
                params[key] = value
        started = self._client.request(METHODS["task_start"], params)
        self._bindings[started["run_id"]] = dict(bind)
        run = Run(self._client, started["run_id"], self._events)
        if on_log is not None:
            run._forward_logs(on_log)
        return run


class Run:
    """A started run (spec section 10.1)."""

    def __init__(self, client: RpcClient, run_id: str, events: _EventHub) -> None:
        self._client = client
        self._events = events
        #: This run's id, which every pause and recording event carries.
        self.id = run_id
        self._ended = False
        self._resume_version = 0
        self._log_forwarder: Callable[[RecordingEvent], None] | None = None
        # A replay reuses its recording's run id, so this run's events are the
        # ones with its id from here on, not an earlier run's (spec 10.4).
        with events.condition:
            self._history_start = len(events.history)

    def _forward_logs(self, on_log: LogHandler) -> None:
        def forward(event: RecordingEvent) -> None:
            if event.get("run_id") != self.id:
                return
            line = log_event(event)
            if line is not None:
                on_log(line)

        self._log_forwarder = forward
        self._events.subscribe(forward, self._history_start)

    def _finish(self) -> None:
        self._ended = True
        self._events.wake()
        if self._log_forwarder is not None:
            self._events.unsubscribe(self._log_forwarder)
            self._log_forwarder = None

    def next(self) -> Pause:
        """Advance until the next pause.

        Raises:
            JevscriptRpcError: if the runtime refuses to advance.
        """
        pause: Pause = self._client.request(METHODS["run_next"], {"run_id": self.id})
        if pause.get("kind") in TERMINAL_PAUSE_KINDS or (
            pause.get("kind") == "error" and not pause.get("retryable", False)
        ):
            self._finish()
        return pause

    def resume(self, payload: Resume) -> None:
        """Answer the open pause (spec section 10.2)."""
        self._client.request(METHODS["run_resume"], {"run_id": self.id, "payload": payload})
        self._resume_version += 1

    def abort(self) -> None:
        """Give up on the run."""
        self._ended = True
        self._client.request(METHODS["run_abort"], {"run_id": self.id})
        self._finish()

    def inject(self, capability: str, message: str) -> None:
        """Send a message to a capability during a ``waiting`` pause."""
        self._client.request(
            METHODS["run_inject"],
            {"run_id": self.id, "capability": capability, "message": message},
        )

    def events(self) -> Iterator[RecordingEvent]:
        """Recording events as they are written (spec section 10.3)."""
        index = self._history_start
        while True:
            with self._events.condition:
                while index >= len(self._events.history) and not self._ended:
                    self._events.condition.wait()
                if index >= len(self._events.history) and self._ended:
                    return
                event = self._events.history[index]
                index += 1
            if event.get("run_id") == self.id:
                yield event

    def logs(self) -> Iterator[LogEvent]:
        """This run's ``log`` lines as they are written (spec section 5.8): the
        ``log`` events of :meth:`events`, as :data:`LogEvent` dictionaries."""
        for event in self.events():
            line = log_event(event)
            if line is not None:
                yield line

    def __iter__(self) -> Iterator[Pause]:
        """Iterate pauses until the run reaches a terminal one."""
        while not self._ended:
            pause = self.next()
            resume_version = self._resume_version
            yield pause
            if pause.get("kind") in TERMINAL_PAUSE_KINDS:
                return
            if (
                pause.get("kind") == "escalate"
                or (
                    pause.get("kind") == "error"
                    and not pause.get("retryable", False)
                )
            ) and self._resume_version == resume_version:
                self._finish()
                return


__all__ = [
    "JevscriptRpcError",
    "Judgment",
    "Program",
    "Run",
    "Task",
    "load",
    "log_event",
]
