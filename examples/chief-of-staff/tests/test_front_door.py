"""The conversational entry point and the tmux JSONL agent boundary."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import pytest

from cos.capabilities.agent import ExternalAdapter
from cos.session import agent_argv, instructions, run_session
from cos.state.home import Home
from cos.tmux_agent import TmuxAgent


def test_agent_session_passes_scoped_instructions_and_home_command(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home = Home(tmp_path / "home")
    prompt = instructions(home)
    assert str(home.root) in prompt and "uv run --project" in prompt
    assert "AGENTS.md" not in prompt and "CLAUDE.md" not in prompt
    monkeypatch.setattr(shutil, "which", lambda name: f"/tools/{name}")
    assert agent_argv("claude", prompt, home)[:2] == ["/tools/claude", "--append-system-prompt"]
    assert agent_argv("codex", prompt, home)[-1] == prompt

    calls: list[tuple[str, object]] = []

    class Watcher:
        def poll(self):
            return None

        def terminate(self):
            calls.append(("terminate", None))

        def wait(self, timeout=None):
            calls.append(("wait", timeout))

    monkeypatch.setattr(subprocess, "Popen", lambda argv, **kw: (calls.append(("watch", argv)), Watcher())[1])
    monkeypatch.setattr(subprocess, "call", lambda argv, **kw: (calls.append(("agent", argv)), 0)[1])
    monkeypatch.setattr(time, "sleep", lambda _: None)
    assert run_session(home, "claude") == 0
    assert [kind for kind, _ in calls] == ["watch", "agent", "terminate", "wait"]


def test_tmux_agent_launch_contract_and_jsonl_errors(tmp_path: Path) -> None:
    named = {"name": "t1", "in": {"path": str(tmp_path)}, "harness": "codex", "model": "gpt-test", "effort": "high", "prompt": "Read brief"}
    argv = TmuxAgent.launch_argv(named)
    assert argv == ["codex", "--model", "gpt-test", "-c", 'model_reasoning_effort="high"', "Read brief"]


@pytest.mark.live
def test_tmux_agent_jsonl_reattach_send_observe_and_stop(tmp_path: Path) -> None:
    if not shutil.which("tmux"):
        pytest.skip("tmux is absent")
    script = tmp_path / "worker.py"
    script.write_text("import sys\nprint('AGENT READY', flush=True)\nfor line in sys.stdin:\n print('RECEIVED ' + line.strip(), flush=True)\n")
    session = f"cos-agent-test-{os.getpid()}"
    env = {"COS_TMUX_SESSION": session}
    adapter = ExternalAdapter([sys.executable, "-m", "cos.tmux_agent"], "agent", "crew", env=env, timeout=20)
    try:
        handle = adapter.verb("spawn", **{"name": "worker", "in": {"path": str(tmp_path)}, "harness": "test", "command": [sys.executable, "-u", str(script)], "prompt": "hello"})
        again = adapter.verb("spawn", **{"name": "worker", "in": {"path": str(tmp_path)}, "harness": "test", "command": [sys.executable, "-u", str(script)], "prompt": "ignored"})
        assert handle["endpoint"]["pane_id"] == again["endpoint"]["pane_id"]
        for _ in range(30):
            if "AGENT READY" in adapter.observe(handle)["tail"]:
                break
            time.sleep(0.1)
        else:
            pytest.fail("agent did not launch in tmux")
        assert adapter.verb("wait", handle, minutes=0)["status"] == "waiting"
        adapter.verb("send", handle, "hello from JSONL")
        for _ in range(30):
            if "RECEIVED hello from JSONL" in adapter.observe(handle)["tail"]:
                break
            time.sleep(0.1)
        else:
            pytest.fail("tmux agent did not receive the message")
        adapter.verb("stop", handle)
        assert adapter.observe(handle)["status"] == "exited"
    finally:
        adapter.close()
        subprocess.run(["tmux", "kill-session", "-t", f"={session}"], capture_output=True, check=False)
