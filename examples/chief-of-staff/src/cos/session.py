"""Launch an interactive agent as the CoS's front door.

The prompt lives beside this example, never in a project's AGENTS.md or
CLAUDE.md. A companion watcher owns the home's single supervisor lock; the
agent talks through the same command layer as the CLI.
"""

from __future__ import annotations

import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

from .state.home import Home


def project_dir() -> Path:
    return Path(__file__).resolve().parents[2]


def instructions(home: Home) -> str:
    path = project_dir() / "agent-session" / "INSTRUCTIONS.md"
    if not path.exists():
        path = Path(__file__).resolve().parent / "agent-session" / "INSTRUCTIONS.md"
    if (project_dir() / "pyproject.toml").exists():
        argv = ["uv", "run", "--project", str(project_dir()), "cos"]
    else:
        argv = [sys.executable, "-m", "cos"]
    prefix = shlex.join([*argv, "--home", str(home.root)])
    identity = home.identity
    return path.read_text().format(home=home.root, command=prefix, name=identity["name"], principal=identity["principal"])


def agent_argv(agent: str, prompt: str, home: Home) -> list[str]:
    executable = shutil.which(agent)
    if executable is None:
        raise ValueError(f"{agent} is not installed; choose claude or codex")
    if agent == "claude":
        return [executable, "--append-system-prompt", prompt, "Start with the briefing and the red box."]
    if agent == "codex":
        return [executable, "-C", str(home.root), prompt]
    raise ValueError("agent must be claude or codex")


def run_session(home: Home, agent: str, *, watch: bool = True) -> int:
    """Run one chat session and stop only its companion watcher on exit."""
    home.init()
    argv = agent_argv(agent, instructions(home), home)
    watcher: subprocess.Popen[str] | None = None
    log = None
    try:
        if watch:
            path = home.state / "session-watcher.log"
            log = path.open("a", encoding="utf-8")
            watcher = subprocess.Popen(  # noqa: S603 - this package's own module
                [sys.executable, "-m", "cos", "--home", str(home.root), "watch"],
                cwd=home.root,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                text=True,
            )
            time.sleep(0.3)
            if watcher.poll() is not None:
                raise RuntimeError(f"the companion watcher exited; inspect {path}")
        return subprocess.call(argv, cwd=home.root)  # noqa: S603 - known installed agent CLI
    finally:
        if watcher is not None and watcher.poll() is None:
            watcher.terminate()
            try:
                watcher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                watcher.kill()
                watcher.wait(timeout=5)
        if log is not None:
            log.close()
