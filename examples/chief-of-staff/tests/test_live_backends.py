"""Live smoke checks against the terminal tools installed on this machine.

Each backend spawns a location, runs a marker command, reads it back, finds
the location again by name, submits a line and closes it. A backend that is
not installed or not usable here is skipped with the reason; Herdr and Orca
also need an explicit opt-in (see `cos.smoke`).
"""

from __future__ import annotations

import pytest

from cos.smoke import smoke


@pytest.mark.live
@pytest.mark.parametrize("backend", ["tmux", "herdr", "zellij", "cmux", "orca"])
def test_backend_smoke(backend: str) -> None:
    report = smoke(backend)
    if report["result"] == "skipped":
        pytest.skip(f"{backend}: {report['reason']}")
    assert report["result"] == "passed", report
