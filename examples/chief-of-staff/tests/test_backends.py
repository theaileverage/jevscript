"""Each terminal backend issues its expected commands.

A scripted runner stands in for the tool: it records every argv and answers
from a table, so these run anywhere. `test_live_backends.py` runs the same
backends against the real tools installed on this machine.
"""

from __future__ import annotations

import json
import subprocess
from typing import Any, Callable

import pytest

from cos import backends
from cos.backends.base import BackendError


class Script:
    def __init__(self, answer: Callable[[list[str]], tuple[int, str]]) -> None:
        self.calls: list[list[str]] = []
        self.answer = answer

    def __call__(self, argv: list[str], **_: Any) -> subprocess.CompletedProcess:
        self.calls.append(argv)
        code, out = self.answer(argv)
        return subprocess.CompletedProcess(argv, code, out, "" if code == 0 else "boom")


def make(name: str, answer, **options) -> tuple[backends.Backend, Script]:
    script = Script(answer)
    backend = backends.make(name, run=script, sleep=lambda _: None, check_path=False, **options)
    return backend, script


def test_detect_follows_configured_order() -> None:
    assert backends.detect({"TMUX": "/tmp/x", "HERDR_ENV": "1"}) == "tmux"
    assert backends.detect({"HERDR_ENV": "1"}) == "herdr"
    assert backends.detect({"CMUX_WORKSPACE_ID": "w"}) == "cmux"
    with pytest.raises(BackendError):
        backends.make("screen")


def test_tmux_spawns_a_named_detached_window_and_targets_its_pane() -> None:
    windows: list[str] = []

    def answer(argv):
        if argv[1] == "has-session":
            return 1, ""
        if argv[1] == "list-windows":
            return 0, "\n".join(windows)
        if argv[1] == "new-window":
            windows.append("t1\t@3\t%7")
            return 0, "@3\t%7\n"
        if argv[1] == "display-message":
            return 0, "%7 0\n"
        if argv[1] == "capture-pane":
            return 0, "hello\n"
        return 0, ""

    tmux, script = make("tmux", answer, session="cos")
    ep = tmux.spawn("t1", "/work")
    assert ep == {"backend": "tmux", "session": "cos", "window": "t1", "window_id": "@3", "pane_id": "%7"}
    assert ["tmux", "new-session", "-d", "-s", "cos", "-c", "/work"] in script.calls
    assert ["tmux", "new-window", "-dP", "-F", "#{window_id}\t#{pane_id}", "-t", "cos:", "-n", "t1", "-c", "/work"] in script.calls
    assert ["tmux", "set-window-option", "-t", "@3", "automatic-rename", "off"] in script.calls
    assert tmux.alive(ep) and tmux.find("t1")["pane_id"] == "%7"
    assert tmux.read(ep, 50) == "hello"
    assert script.calls[-1] == ["tmux", "capture-pane", "-p", "-J", "-t", "%7", "-S", "-50"]
    tmux.type_text(ep, "echo hi")
    assert script.calls[-1] == ["tmux", "send-keys", "-t", "%7", "-l", "echo hi"]
    with pytest.raises(BackendError):
        tmux.spawn("t1", "/work")  # never two windows with one name


def test_herdr_uses_a_labelled_workspace_native_busy_and_json_presence() -> None:
    state = {"workspaces": [], "tabs": []}

    def answer(argv):
        args = argv[1:]
        if args[:2] == ["workspace", "list"]:
            return 0, json.dumps({"result": {"workspaces": state["workspaces"]}})
        if args[:2] == ["workspace", "create"]:
            state["workspaces"].append({"workspace_id": "w9", "label": "cos"})
            return 0, json.dumps({"result": {"workspace": {"workspace_id": "w9"}, "tab": {"tab_id": "w9:t1"}, "root_pane": {"pane_id": "w9:p1"}}})
        if args[:2] == ["tab", "create"]:
            state["tabs"].append({"tab_id": "w9:t2", "label": "cos-t1"})
            return 0, json.dumps({"result": {"tab": {"tab_id": "w9:t2"}, "root_pane": {"pane_id": "w9:p2"}}})
        if args[:2] == ["tab", "list"]:
            return 0, json.dumps({"result": {"tabs": [{"tab_id": "w9:t1", "label": "1"}] + state["tabs"]}})
        if args[:2] == ["pane", "list"]:
            return 0, json.dumps({"result": {"panes": [{"pane_id": "w9:p2", "tab_id": "w9:t2"}]}})
        if args[:2] == ["pane", "get"]:
            if args[2] == "w9:gone":
                return 1, json.dumps({"error": {"code": "pane_not_found"}})
            return 0, json.dumps({"result": {"pane": {"pane_id": args[2]}}})
        if args[:2] == ["agent", "get"]:
            return 0, json.dumps({"result": {"agent": {"agent_status": "working"}}})
        if args[:2] == ["pane", "read"]:
            return 0, "line1\nline2\n"
        return 0, json.dumps({"result": {}})

    herdr, script = make("herdr", answer)
    ep = herdr.spawn("t1", "/work")
    assert ep["pane_id"] == "w9:p2" and ep["label"] == "cos-t1"
    assert ["herdr", "workspace", "create", "--cwd", "/work", "--label", "cos", "--no-focus"] in script.calls
    assert ["herdr", "pane", "close", "w9:p1"] in script.calls, "the seed tab of a fresh workspace is pruned"
    assert herdr.native_busy(ep) is True
    assert herdr.busy(ep, "never-matches") is True, "native state wins over screen scraping"
    assert herdr.alive(ep) and not herdr.alive({"pane_id": "w9:gone"})
    assert herdr.read(ep, 1) == "line2"
    assert ["herdr", "pane", "read", "w9:p2", "--source", "recent-unwrapped", "--lines", "200"] in script.calls
    herdr.run_line(ep, "ls")
    assert script.calls[-1] == ["herdr", "pane", "run", "w9:p2", "ls"]
    herdr.key(ep, "C-c")
    assert script.calls[-1] == ["herdr", "pane", "send-keys", "w9:p2", "ctrl+c"]
    assert herdr.find("t1")["pane_id"] == "w9:p2"


def test_herdr_refuses_to_guess_between_two_labelled_workspaces() -> None:
    spaces = [{"workspace_id": "w1", "label": "cos"}, {"workspace_id": "w2", "label": "cos"}]
    herdr, _ = make("herdr", lambda argv: (0, json.dumps({"result": {"workspaces": spaces}})))
    with pytest.raises(BackendError, match="refusing to guess"):
        herdr.spawn("t1", "/work")


def test_zellij_requires_pane_ids_and_closes_whole_tabs() -> None:
    old, _ = make("zellij", lambda argv: (0, "zellij 0.43.1\n"))
    ok, why = old.available()
    assert not ok and "0.44" in why

    def answer(argv):
        if argv[1] == "--version":
            return 0, "zellij 0.44.0\n"
        if argv[1] == "list-sessions":
            return 0, "cos\n"
        action = argv[4:]
        if action[0] == "list-tabs":
            return 0, json.dumps([{"name": "main", "tab_id": 0, "active": True}])
        if action[0] == "new-tab":
            return 0, "4\n"
        if action[0] == "list-panes":
            return 0, json.dumps([{"id": 11, "tab_id": 4, "is_plugin": False}, {"id": 1, "tab_id": 4, "is_plugin": True}])
        return 0, ""

    zj, script = make("zellij", answer)
    assert zj.available() == (True, "")
    ep = zj.spawn("t1", "/work")
    assert ep["pane_id"] == 11 and ep["tab_id"] == 4
    assert ["zellij", "--session", "cos", "action", "go-to-tab-by-id", "0"] in script.calls, "focus is restored"
    zj.key(ep, "C-c")
    assert script.calls[-1] == ["zellij", "--session", "cos", "action", "send-keys", "--pane-id", "11", "Ctrl c"]
    zj.close(ep)
    assert ["zellij", "--session", "cos", "action", "close-tab-by-id", "4"] in script.calls


def test_cmux_addresses_workspaces_by_title_and_surface_uuid() -> None:
    made: list[str] = []

    def answer(argv):
        args = argv[1:]
        if args[0] == "ping":
            return 0, "PONG\n"
        if args[:2] == ["workspace", "list"]:
            return 0, json.dumps({"workspaces": [{"id": "W-1", "title": t} for t in made]})
        if args[0] == "new-workspace":
            made.append(args[2])
            return 0, ""
        if args[0] == "list-panes":
            return 0, json.dumps({"panes": [{"selected_surface_id": "S-1", "surface_ids": ["S-1"]}]})
        if args[0] == "read-screen":
            return 0, json.dumps({"text": "a\nb\n"})
        return 0, ""

    cmux, script = make("cmux", answer)
    assert cmux.available() == (True, "")
    ep = cmux.spawn("t1", "/work")
    assert ep == {"backend": "cmux", "workspace": "W-1", "surface": "S-1", "name": "t1"}
    assert ["cmux", "new-workspace", "--name", "cos-t1", "--cwd", "/work", "--focus", "false", "--id-format", "uuids"] in script.calls
    assert cmux.read(ep, 1) == "b" and cmux.alive(ep)
    cmux.type_text(ep, "hi")
    assert script.calls[-1] == ["cmux", "send", "--workspace", "W-1", "--surface", "S-1", "--", "hi"]


def test_orca_owns_worktrees_and_terminals() -> None:
    def answer(argv):
        args = argv[1:]
        if args[:2] == ["repo", "show"]:
            return 0, json.dumps({"ok": False, "error": {"message": "not found"}})
        if args[:2] == ["worktree", "create"]:
            return 0, json.dumps({"ok": True, "result": {"worktree": {"path": "/wt/t1", "id": "wt1", "branch": "cos/t1"}}})
        if args[:2] == ["terminal", "create"]:
            return 0, json.dumps({"ok": True, "result": {"terminal": {"handle": "term_1"}}})
        if args[:2] == ["terminal", "read"]:
            return 0, json.dumps({"ok": True, "result": {"terminal": {"tail": ["x", "y"]}}})
        return 0, json.dumps({"ok": True, "result": {}})

    orca, script = make("orca", answer)
    assert orca.provides_worktrees
    wt = orca.create_worktree("/proj", "cos/t1", "main")
    assert wt["path"] == "/wt/t1"
    assert ["orca", "repo", "add", "--path", "/proj", "--json"] in script.calls
    ep = orca.spawn("t1", "/wt/t1")
    assert ep["terminal"] == "term_1" and orca.read(ep) == "x\ny"
    orca.key(ep, "Enter")
    assert script.calls[-1] == ["orca", "terminal", "send", "--terminal", "term_1", "--text", "", "--enter", "--json"]
    with pytest.raises(BackendError):
        orca.key(ep, "Escape")  # Orca's CLI has no Escape
    orca.remove_worktree("/wt/t1")
    assert script.calls[-1] == ["orca", "worktree", "rm", "--worktree", "path:/wt/t1", "--json"], "never --force"


def test_submit_types_once_and_retries_only_enter() -> None:
    fake = backends.make("fake")
    ep = fake.spawn("sub", "/tmp")
    keys: list[str] = []
    original = fake.key

    def key(ep_, k):
        keys.append(k)
        original(ep_, k)

    fake.key = key  # type: ignore[method-assign]
    assert fake.submit(ep, "hello\nworld") == "sent"
    assert fake.pane(ep)["screen"][-1] == "$ hello world", "a multi-line steer arrives as one line"
    assert keys == ["Enter"]
