"""Shared fixtures: a git project, and a home bound to the offline Jev and the
fake agent adapter process. Nothing here needs a key or a network."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any, Callable, Iterator

import pytest

SRC = Path(__file__).resolve().parents[1] / "src"
sys.path.insert(0, str(SRC))

from cos.state.home import Home  # noqa: E402
from cos.host import Host  # noqa: E402
from cos.episode import JevSession
from cos.jevbin import find_jevscript  # noqa: E402

JEV = Path(__file__).resolve().parents[1] / "jev"

try:  # the SDK's own `load` reads JEVSCRIPT_BIN
    import os

    os.environ.setdefault("JEVSCRIPT_BIN", find_jevscript())
except Exception:  # noqa: BLE001 - the jevscript_bin fixture skips with the reason
    pass


def git(repo: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture(scope="session")
def jevscript_bin() -> str:
    try:
        return find_jevscript()
    except Exception as error:  # noqa: BLE001
        pytest.skip(str(error))


@pytest.fixture
def project(tmp_path: Path) -> Path:
    repo = tmp_path / "proj"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    (repo / "README.md").write_text("# Proj\n")
    git(repo, "add", ".")
    git(repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "initial")
    return repo


def fake_agent_entry(tmp_path: Path, scripts: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    env = {"COS_FAKE_AGENT_STATE": str(tmp_path / "agents.json"), "PYTHONPATH": str(SRC)}
    if scripts is not None:
        path = tmp_path / "agent-scripts.json"
        path.write_text(json.dumps(scripts))
        env["COS_FAKE_AGENT_SCRIPTS"] = str(path)
    return {"command": [sys.executable, "-m", "cos.fake_agent"], "env": env, "timeout": 30}


BASE_RULES: list[dict[str, Any]] = [
    {"match": {"id": "^work$"}, "answer": {"choice": "ship"}},
    {"match": {"id": "^project$"}, "answer": {"choice": "i0"}},
    {"match": {"id": "^home$"}, "answer": {"choice": "i0"}},
]


@pytest.fixture
def make_host(tmp_path: Path, project: Path, jevscript_bin: str) -> Iterator[Callable[..., Host]]:
    hosts: list[Host] = []

    def build(
        rules: list[dict[str, Any]] | None = None,
        scripts: list[dict[str, Any]] | None = None,
        mode: str = "local-only",
        yolo: bool = True,
        config: dict[str, Any] | None = None,
        home_dir: Path | None = None,
        register: bool = True,
    ) -> Host:
        home = Home(home_dir or tmp_path / "home").init()
        home.set_config("jev.mode", "fake")
        home.set_config("adapters.crew", fake_agent_entry(tmp_path, scripts))
        home.set_config("mates_inline", True)
        for key, value in (config or {}).items():
            home.set_config(key, value)
        session = JevSession(home, mode="fake", rules=(rules or []) + BASE_RULES)
        host = Host(home, session=session, echo=False)
        if register and not host.registry.projects():
            host.registry.add_project("proj", str(project), "the test project", mode=mode, yolo=yolo)
        hosts.append(host)
        return host

    yield build
    for host in hosts:
        host.close()


def run_until(host: Host, predicate: Callable[[], bool], ticks: int = 12) -> None:
    for _ in range(ticks):
        if predicate():
            return
        host.tick()
    assert predicate(), "the condition never held"
