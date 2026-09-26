"""The types the SDK surface is written in.

Pauses, adapters and recording events arrive as plain dictionaries off the
wire; the aliases and protocols here name their shapes. The IR is deliberately
absent: the SDKs never expose it to application code (spec section 11.1).
"""

from __future__ import annotations

from typing import Any, Callable, Literal, Protocol, runtime_checkable

#: The seven pause kinds (spec section 10.2).
PauseKind = Literal["confirm", "escalate", "waiting", "budget", "error", "stopped", "done"]

#: The four capability kinds (spec section 9).
CapabilityKind = Literal["agent", "person", "llm", "tool"]

#: A pause, as it comes off the wire. ``kind`` decides the other fields.
Pause = dict[str, Any]

#: What a host resumes a pause with (spec section 10.2): ``{"answer": ...}`` for
#: ``confirm``, ``{"resume": True}`` for ``escalate``, ``{"extend": {...}}`` for
#: ``budget``, ``{"retry": True}`` for a retryable ``error``, ``{}`` for
#: ``waiting``.
Resume = dict[str, Any]

#: An opaque reference to something an adapter owns.
Handle = dict[str, Any]

#: What an ``agent`` observation carries at least (spec section 9.1).
Observation = dict[str, Any]

#: What a ``tool`` adapter may publish about itself (spec section 9.4):
#: ``{"verbs": {"<name>": {"params": [...], "returns": "<type>"}}}``. At
#: ``task.start`` the runtime checks every verb the program references against
#: it and refuses to start with ``verb_missing`` if one is absent.
ToolManifest = dict[str, Any]

#: What a machine call returns (spec section 7.8): ``state``, ``steps``,
#: ``done``, ``verified`` and ``events``. ``verified`` is true only when the
#: transition into the terminal state carried a ``when`` guard, which is code.
MachineResult = dict[str, Any]

#: ``sample: bool | {"seed": int}`` (spec section 6.11).
Sample = bool | dict[str, int]

#: One recording event (spec section 10.3).
RecordingEvent = dict[str, Any]

#: The four ``log`` levels, lowest first (spec section 5.8).
LogLevel = Literal["debug", "info", "warn", "error"]

#: The levels in order, for filtering by severity.
LOG_LEVELS: tuple[str, ...] = ("debug", "info", "warn", "error")

#: One ``log`` line (spec section 5.8): ``level``, ``message`` (the logged
#: value's text form), ``fields`` (a record whose structured values keep their
#: ``$jev`` tags), ``task`` (the task, ``machine:<name>`` or judgment it ran
#: under) and ``source`` (its span in the ``.jev`` file). A line from a run also
#: carries ``run_id`` and ``seq``; a judgment run alone has neither. The host
#: stream carries logs in full: ``redact`` governs only the stored recording.
LogEvent = dict[str, Any]

#: A host callback that receives each ``log`` line as it is written.
LogHandler = Callable[[LogEvent], None]

#: The kinds that end a run. ``escalate`` ends it unless the host resumes, and
#: ``error`` unless the code is retryable.
TERMINAL_PAUSE_KINDS: frozenset[str] = frozenset({"done", "stopped"})


@runtime_checkable
class Adapter(Protocol):
    """An adapter, as the host binds it.

    The runtime asks for ``call(verb, args)`` and, for ``agent`` kinds,
    ``observe(handle)`` (spec section 9.5). Adapters must return structured
    records, never opaque blobs, so that ``shape`` and ``trail`` can work on
    them.
    """

    kind: CapabilityKind
    capability: str

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        """Run one verb."""
        ...
