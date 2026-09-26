"""Terminal backends behind one interface (``base.Backend``).

Herdr is the default; tmux, Zellij, cmux and Orca are alternatives, and
``fake`` serves tests. Selection follows this order: an explicit name,
then the runtime the host is itself running inside ($TMUX, HERDR_ENV=1,
CMUX_WORKSPACE_ID), then Herdr when it is installed, then tmux. Zellij and
Orca are only ever chosen explicitly.
"""

from __future__ import annotations

import os
import shutil
from typing import Any

from .base import Backend, BackendError, strip_ansi, screen_hash
from .cmux import CmuxBackend
from .fake import FakeBackend
from .herdr import HerdrBackend
from .orca import OrcaBackend
from .tmux import TmuxBackend
from .zellij import ZellijBackend

BACKENDS: dict[str, type[Backend]] = {
    "herdr": HerdrBackend,
    "tmux": TmuxBackend,
    "zellij": ZellijBackend,
    "cmux": CmuxBackend,
    "orca": OrcaBackend,
    "fake": FakeBackend,
}


def detect(env: dict[str, str] | None = None) -> str:
    env = dict(os.environ if env is None else env)
    if env.get("TMUX"):
        return "tmux"
    if env.get("HERDR_ENV") == "1":
        return "herdr"
    if env.get("CMUX_WORKSPACE_ID"):
        return "cmux"
    if shutil.which("herdr"):
        return "herdr"
    return "tmux"


def make(name: str | None, **options: Any) -> Backend:
    chosen = detect() if name in (None, "", "auto") else name
    try:
        cls = BACKENDS[chosen]
    except KeyError:
        raise BackendError(f"unknown backend `{chosen}`; known: {', '.join(BACKENDS)}", retryable=False) from None
    return cls(**options)


__all__ = ["BACKENDS", "Backend", "BackendError", "detect", "make", "screen_hash", "strip_ansi"]
