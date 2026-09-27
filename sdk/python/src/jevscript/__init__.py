"""Python host SDK for Jevscript.

Drives the ``jevscript`` runtime over JSON-RPC on stdio (spec section 11.5).
``spec/jevscript-language-specification.md`` is the authority for every name
here.
"""

from .client import Judgment, Program, Run, Task, load, log_event
from .rpc import HOST_METHODS, METHODS, JevscriptRpcError, RpcClient
from .subprocess_adapter import SubprocessAdapterError, SubprocessAgentAdapter, subprocess_agent
from .types import (
    LOG_LEVELS,
    TERMINAL_PAUSE_KINDS,
    Adapter,
    LogEvent,
    LogHandler,
    LogLevel,
    MachineResult,
    Observation,
    Pause,
    PauseKind,
    Resume,
    Sample,
    ToolManifest,
)

__version__ = "0.1.3"

__all__ = [
    "Adapter",
    "HOST_METHODS",
    "JevscriptRpcError",
    "Judgment",
    "LOG_LEVELS",
    "LogEvent",
    "LogHandler",
    "LogLevel",
    "MachineResult",
    "METHODS",
    "Observation",
    "Pause",
    "PauseKind",
    "Program",
    "Resume",
    "Run",
    "Sample",
    "ToolManifest",
    "RpcClient",
    "SubprocessAdapterError",
    "SubprocessAgentAdapter",
    "TERMINAL_PAUSE_KINDS",
    "Task",
    "load",
    "log_event",
    "subprocess_agent",
]
