"""Second mates: scoped Chief of Staff instances with homes of their own.

A second mate is a full home
(``mates/<name>`` under the parent) with its own backlog, workers, ledger,
playbooks and session lock, and a scope. Intake routes in-scope requests to
it (``homes`` in ``intake.assess``); the parent drops the request into the
mate's request queue and keeps the mate's own watcher running in a terminal
backend. A mate is idle by default and acts only on routed work. It reports
finished work up to ``state/mates/<name>.log`` in the parent, which bearings
reads; the parent never reaches into the mate's workers.
"""

from __future__ import annotations

import shlex
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .state.home import Home, append_jsonl, iso, read_json, read_jsonl, write_json

if TYPE_CHECKING:  # pragma: no cover
    from .host import Host


class Mates:
    def __init__(self, host: "Host") -> None:
        self.host = host

    def _home(self, name: str) -> Home:
        for mate in self.host.registry.mates():
            if mate["name"] == name:
                return Home(mate["home"])
        raise KeyError(f"no second mate `{name}`")

    def forward(self, name: str, request: dict[str, Any]) -> None:
        child = self._home(name).init()
        write_json(child.state / "requests" / f"{request['id']}.json", {**request, "from_parent": True, "at": iso()})
        self.host.decisions.digest(f"Passed '{request['text'][:60]}' to second mate {name}.")
        self.ensure_running(name)

    def command(self, name: str) -> str:
        child = self._home(name)
        return f"{shlex.quote(sys.executable)} -m cos --home {shlex.quote(str(child.root))} watch"

    def ensure_running(self, name: str) -> dict[str, Any] | None:
        """Start the mate's watcher in a terminal if it is not already live.
        With no terminal backend configured (tests), the parent's own tick
        drives the mate instead (see ``tick``)."""
        if self.host.home.config.get("mates_inline", False):
            return None
        record_path = self.host.home.state / "mates" / f"{name}.json"
        record = read_json(record_path, {})
        terminal = self.host.terminal()
        endpoint = record.get("endpoint")
        if endpoint and terminal.alive(endpoint):
            return endpoint
        found = terminal.find(f"mate-{name}")
        if found:
            write_json(record_path, {"endpoint": found, "at": iso()})
            return found
        endpoint = terminal.spawn(f"mate-{name}", str(self._home(name).root))
        terminal.run_line(endpoint, self.command(name))
        write_json(record_path, {"endpoint": endpoint, "at": iso()})
        return endpoint

    def tick(self) -> None:
        """Inline mode: run one wake of each mate that has routed work."""
        if not self.host.home.config.get("mates_inline", False):
            return
        from .host import Host

        for mate in self.host.registry.mates():
            child = Home(mate["home"])
            if not list((child.state / "requests").glob("*.json")) and not any(True for _ in (child.state / "workers").glob("*.json")):
                continue
            host = Host(child, session=self.host.session, crew=self.host.crew.adapter, writer=self.host.writer, delivery=self.host.delivery, echo=False)
            try:
                host.tick()
            finally:
                host.program.close()  # the shared adapter and Jev session stay open for the parent

    def report_up(self, task_id: str, outcome: str, title: str) -> None:
        parent = read_json(self.host.home.data / "parent.json", None)
        if not parent:
            return
        append_jsonl(Path(parent["home"]) / "state" / "mates" / f"{parent['name']}.log", {"at": iso(), "task": task_id, "title": title, "outcome": outcome})

    def summaries(self) -> list[dict[str, Any]]:
        rows = []
        for mate in self.host.registry.mates():
            child = Home(mate["home"])
            reports = read_jsonl(self.host.home.state / "mates" / f"{mate['name']}.log")
            decisions = read_json(child.state / "decisions.json", {})
            rows.append(
                {
                    "name": mate["name"],
                    "scope": mate["scope"],
                    "open_decisions": sum(1 for d in decisions.values() if d.get("answer") is None),
                    "in_flight": sum(1 for i in read_json(child.data / "backlog.json", []) if i["status"] == "in_flight"),
                    "recent": reports[-3:],
                }
            )
        return rows
