"""The real JSONL child boundary, using the same executable as JS tests."""

from __future__ import annotations

import pathlib
import shutil

import pytest

from jevscript import SubprocessAdapterError, load, subprocess_agent

ROOT = pathlib.Path(__file__).resolve().parents[3]
FIXTURE = ROOT / "sdk/js/test/fixtures/agent-adapter.mjs"
BINARY = ROOT / "target/debug/jevscript"


@pytest.mark.skipif(shutil.which("node") is None, reason="Node is absent")
def test_subprocess_agent_forwards_calls_observations_and_retryability():
    with subprocess_agent("node", [str(FIXTURE)]) as adapter:
        adapter.capability = "worker"
        handle = adapter.call("spawn", {"named": {"prompt": "hello"}})
        assert handle == {"id": "fake-agent-1", "capability": "worker"}
        assert adapter.observe(handle)["id_seen"] == "fake-agent-1"
        assert adapter.call("send", {"positional": [handle, "more"]})["capability"] == "worker"
        with pytest.raises(SubprocessAdapterError) as caught:
            adapter.call("fail", {})
        assert str(caught.value) == "try later"
        assert caught.value.retryable is True
        assert adapter.call("stop", {}) is None
    with pytest.raises(SubprocessAdapterError, match="closed"):
        adapter.call("stop", {})


@pytest.mark.skipif(shutil.which("node") is None or not BINARY.exists(), reason="Node or runtime binary is absent")
def test_subprocess_agent_forwards_through_runtime():
    source = ROOT / "adapters/core/test/fixtures/sdk_subprocess.jev"
    with subprocess_agent("node", [str(FIXTURE)]) as adapter:
        with load(str(source), bin=str(BINARY)) as program:
            pause = program.task("main").start(bind={"dev": adapter}).next()
            assert pause["kind"] == "done"
            assert pause["outputs"]["message"] == "finished"
