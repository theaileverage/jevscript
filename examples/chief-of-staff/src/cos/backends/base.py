"""The shared adapter interface every terminal backend implements.

A backend owns terminal locations (a tmux window, a Herdr tab, a Zellij tab,
a cmux workspace, an Orca terminal) and nothing else: it can make one, find it
again by name after a restart, read its screen, type into it, press keys, say
whether it is alive and whether the agent in it is mid-turn, and close it.
The `agent` capability (``capabilities.AgentCapability``) sits on top and adds
harness launch lines, trust dialogs, observation records and bounded waits.

Every shell-out goes through an injected runner so unit tests can check the
exact command lines without the tool installed, and every blocking loop is
bounded: the SDK cannot abort a step that is inside an adapter call.
"""

from __future__ import annotations

import hashlib
import re
import subprocess
import time
from abc import ABC, abstractmethod
from typing import Any, Callable

Runner = Callable[..., subprocess.CompletedProcess]

ANSI = re.compile(r"\x1b\[[0-9;:?]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]")

#: Keys the Chief of Staff presses. Each backend maps them to its own names.
KEYS = ("Enter", "Escape", "C-c", "C-u")


class BackendError(RuntimeError):
    """A backend command failed. ``retryable`` travels to the runtime as an
    ``adapter_error``'s retryability (spec section 12)."""

    def __init__(self, message: str, retryable: bool = True) -> None:
        super().__init__(message)
        self.retryable = retryable


def default_runner(argv: list[str], *, input: str | None = None, timeout: float = 30.0, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, input=input, capture_output=True, text=True, timeout=timeout, check=False, env=env)
    except FileNotFoundError as error:
        raise BackendError(f"`{argv[0]}` is not installed", retryable=False) from error
    except subprocess.TimeoutExpired as error:
        raise BackendError(f"`{' '.join(argv[:3])}` timed out after {timeout:.0f}s") from error


def strip_ansi(text: str) -> str:
    return ANSI.sub("", text)


def screen_hash(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


class Backend(ABC):
    """One terminal runtime. Endpoints are plain dicts so they persist in a
    handle and survive a host restart; ``find`` recovers one by name."""

    name: str = ""
    #: True when the backend creates isolated copies itself (Orca).
    provides_worktrees: bool = False
    settle = 0.3
    enter_sleep = 0.4
    retries = 3

    def __init__(self, run: Runner = default_runner, sleep: Callable[[float], None] = time.sleep, **options: Any) -> None:
        self._run = run
        self._sleep = sleep
        self.options = options

    # -- plumbing -----------------------------------------------------------

    def sh(self, argv: list[str], *, check: bool = True, timeout: float = 30.0, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
        result = self._run(argv, timeout=timeout, env=env)
        if check and result.returncode != 0:
            detail = (result.stderr or result.stdout or "").strip().splitlines()
            raise BackendError(f"{self.name}: `{' '.join(argv[:4])}` failed: {detail[-1] if detail else result.returncode}")
        return result

    # -- the interface --------------------------------------------------------

    @abstractmethod
    def available(self) -> tuple[bool, str]:
        """Whether this backend can spawn here, and why not."""

    @abstractmethod
    def spawn(self, name: str, cwd: str) -> dict[str, Any]:
        """A fresh shell location named ``name`` in ``cwd``."""

    @abstractmethod
    def find(self, name: str) -> dict[str, Any] | None:
        """The live location named ``name``, if one exists."""

    @abstractmethod
    def alive(self, ep: dict[str, Any]) -> bool: ...

    @abstractmethod
    def read(self, ep: dict[str, Any], lines: int = 200) -> str: ...

    @abstractmethod
    def type_text(self, ep: dict[str, Any], text: str) -> None:
        """Type ``text`` without submitting it."""

    @abstractmethod
    def key(self, ep: dict[str, Any], key: str) -> None: ...

    @abstractmethod
    def close(self, ep: dict[str, Any]) -> bool: ...

    def native_busy(self, ep: dict[str, Any]) -> bool | None:
        """The backend's own reading of the agent's state, when it has one."""
        return None

    def run_line(self, ep: dict[str, Any], line: str) -> None:
        """Type a shell line and press Enter (launch lines, not agent input)."""
        self.type_text(ep, line)
        self._sleep(self.settle)
        self.key(ep, "Enter")

    # -- shared behaviour -----------------------------------------------------

    def busy(self, ep: dict[str, Any], pattern: str = "") -> bool | None:
        native = self.native_busy(ep)
        if native is not None:
            return native
        if not pattern:
            return None
        tail = [line for line in strip_ansi(self.read(ep, 40)).splitlines() if line.strip()][-12:]
        return bool(re.search(pattern, "\n".join(tail), re.IGNORECASE | re.MULTILINE))

    def submit(self, ep: dict[str, Any], text: str, busy_pattern: str = "") -> str:
        """Type one line into an agent's composer and make sure it was taken.

        Type once; retry only Enter. The line counts as
        submitted once Enter changes the screen (the composer empties and the
        text moves into the transcript) or the agent turns busy. Returns
        ``sent``, ``busy`` or ``pending``.
        """
        line = " ".join(text.splitlines()).strip()
        if not line:
            return "sent"
        self.type_text(ep, line)
        self._sleep(self.settle if not line.startswith("/") else 1.2)
        before = strip_ansi(self.read(ep, 40))
        for _ in range(self.retries):
            self.key(ep, "Enter")
            self._sleep(self.enter_sleep)
            after = strip_ansi(self.read(ep, 40))
            if busy_pattern and re.search(busy_pattern, after, re.IGNORECASE | re.MULTILINE):
                return "busy"
            if after != before:
                return "sent"
        return "pending"
