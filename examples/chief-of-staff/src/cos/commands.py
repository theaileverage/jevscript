"""The command layer: everything the principal can ask of the CoS.

The CLI is one front end over this class; an agent session acting as the
chat (a Claude Code or Codex session calling the CoS as a tool) is
meant to be another. Every method returns a ``Reply``: a short outcome summary
for the person (written by the `llm` capability through
``escalation.summarize`` when it describes something that happened) plus
structured ``data`` a program can read.

Methods that only read or record (answer, away, remember, playbooks...) need
no runtime; methods that act (say, tick, steer, learn) start the host lazily
and take the home's session lock, so they never race a running ``cos watch``:
if a watcher holds the lock, a request is queued and the watcher acts on it.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from types import SimpleNamespace
from typing import TYPE_CHECKING, Any, Callable

from .state.backlog import Backlog
from .state.decisions import Decisions
from .state.home import Home, LockHeld
from .learning import Playbooks
from .state.memory import Memory
from .state.registry import Registry

if TYPE_CHECKING:  # pragma: no cover
    from .host import Host


@dataclass
class Reply:
    text: str
    data: Any = field(default=None)


class _HomeOnly:
    def __init__(self, home: Home) -> None:
        self.home = home


class Commands:
    def __init__(self, home: Home | str | Path, host_factory: Callable[[Home], "Host"] | None = None) -> None:
        self.home = home if isinstance(home, Home) else Home(home)
        self.home.init()
        self._factory = host_factory
        self._host: "Host | None" = None
        self.registry = Registry(self.home)
        self.backlog = Backlog(self.home)
        self.decisions = Decisions(self.home, echo=False)
        self.playbook_registry = Playbooks(_HomeOnly(self.home))  # type: ignore[arg-type]

    # -- the host, started only when a command acts --------------------------------------

    @property
    def host(self) -> "Host":
        if self._host is None:
            if self._factory is not None:
                self._host = self._factory(self.home)
            else:
                from .host import Host

                self._host = Host(self.home)
        return self._host

    def close(self) -> None:
        if self._host is not None:
            self._host.close()
            self._host = None

    def __enter__(self) -> "Commands":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    # -- talking to it ------------------------------------------------------------

    def say(self, text: str, *, after: list[str] | None = None, project: str | None = None, channel: str | None = None, message_id: str | None = None, reply_to: str | None = None, native_thread: str | None = None) -> Reply:
        """Hand over a request. With no watcher running, the wakes it causes
        run now and the reply says what became of it."""
        request = self.host.submit(text, after=after, project=project, source="cos", channel=channel, message_id=message_id, reply_to=reply_to, native_thread=native_thread)
        if request.get("duplicate") and request["receipt"]["status"] == "done":
            receipt = request["receipt"]
            if receipt.get("route") == "rejected":
                return Reply(f"Rejected message {request['message_id']}: {receipt['reason']}", receipt)
            return Reply(f"Already received message {request['message_id']} in thread {receipt.get('thread_id') or 'unresolved'} (task {receipt.get('task_id') or 'none'}).", receipt)
        try:
            with self.home.lock():
                summary = self.host.tick()
        except LockHeld:
            return Reply("Got it. The running session will pick this up on its next wake.", {"request": request["id"], "queued": True})
        return self._reply_for(summary, request["id"])

    def _reply_for(self, summary: dict[str, Any], only: str | None = None) -> Reply:
        lines = []
        for handled in summary.get("handled", []):
            result = handled.get("result") or {}
            if handled["kind"] == "request" and (only is None or handled["subject"] == only):
                if result.get("route") == "reassess":
                    continue
                if handled.get("paused"):
                    question = (self.decisions.all().get(handled["paused"]) or {}).get("question", "")
                    lines.append(f"I need one answer first: {question} Reply with `cos answer {handled['paused']} <answer>`")
                elif result:
                    lines.append(self._describe(result))
            elif handled["kind"] == "dispatch":
                started = [s["id"] for s in result.get("started", []) if not s.get("blocked")]
                if started:
                    lines.append(f"Started {len(started)} worker{'s' if len(started) != 1 else ''}: {', '.join(started)}")
            elif handled["kind"] == "worker" and handled.get("paused"):
                question = (self.decisions.all().get(handled["paused"]) or {}).get("question", "")
                lines.append(f"Decision needed: {question}")
        for subject in summary.get("resumed", []):
            lines.append(f"Carried on with {subject} after your answer")
        return Reply(self.host.reply(lines) if lines else "Nothing needed doing.", summary)

    def _describe(self, d: dict[str, Any]) -> str:
        route = d.get("route")
        title = d.get("title", "the request")
        if route == "backlog":
            return f"Queued '{title}' for {d.get('project')} as {d.get('kind')} work in thread {d.get('thread_id')} (task {d.get('task_id')})"
        if route == "continue":
            return f"Continued thread {d.get('thread_id')} on task {d.get('task_id') or 'none'}"
        if route == "ask_thread":
            return f"Could not continue the referenced thread: {d.get('reason')}"
        if route == "playbook":
            return f"Matched the learned playbook {d.get('playbook')} for '{title}' in thread {d.get('thread_id')} (task {d.get('task_id')})"
        if route == "memory":
            return f"Recorded it as a standing preference in thread {d.get('thread_id')}"
        if route == "status":
            return f"{self.status().text} Thread {d.get('thread_id')}."
        if route == "mate":
            return f"Passed '{title}' to minister {d.get('mate')} in thread {d.get('thread_id')}"
        if route == "declined":
            return f"Dropped '{title}' as you asked"
        return f"Could not place '{title}': {d.get('reason') or 'it stayed unclear'}"

    def status(self) -> Reply:
        from .bearings import headline

        workers = [w for w in _workers(self.home)]
        queued = [i for i in self.backlog.items() if i["status"] == "queued"]
        open_decisions = self.decisions.open()
        text = headline(len(workers), len(queued), len(open_decisions))
        return Reply(text, {"workers": [w["id"] for w in workers], "queued": [i["id"] for i in queued], "decisions": [d["key"] for d in open_decisions]})

    def briefing(self) -> Reply:
        from .bearings import render
        from .mates import Mates
        from .state.workers import Workers

        # The briefing reads durable records; starting a Host would also start
        # Jev (and write fake profiles in offline homes) before any work runs.
        reader = SimpleNamespace(
            home=self.home,
            registry=self.registry,
            backlog=self.backlog,
            decisions=self.decisions,
            workers=Workers(self.home),
            learning=SimpleNamespace(playbooks=self.playbook_registry),
        )
        reader.mates = Mates(reader)
        return Reply(render(reader))

    def decisions_open(self) -> Reply:
        rows = self.decisions.open()
        if not rows:
            return Reply("Nothing needs you.", [])
        text = "\n".join(f"[{d['key']}] {d['question']} ({' / '.join(d['options']) or 'answer in text'})" for d in rows)
        return Reply(text, rows)

    def answer(self, key: str, answer: str) -> Reply:
        """Answer a question; the episode waiting on it resumes right away
        when no watcher is running, else on the watcher's next tick."""
        row = self.decisions.answer(key, answer)
        try:
            with self.home.lock():
                summary = self.host.tick()
        except LockHeld:
            return Reply(f"Answered: {answer}. The running session carries on with it.", row)
        reply = self._reply_for(summary)
        return Reply(f"Answered: {answer}. {reply.text}", {"decision": row, **(reply.data or {})})

    def steer(self, task_id: str, text: str) -> Reply:
        result = self.host.fleet.v_steer(task_id, text)
        return Reply(f"Passed your instruction to {task_id}." if result["rung"] else f"Saved your instruction for {task_id}; it has no live session to tell yet.", result)

    def remember(self, text: str) -> Reply:
        fresh = Memory(self.home).remember_preference(text)
        return Reply("Remembered." if fresh else "Already remembered.", {"new": fresh})

    # -- modes ----------------------------------------------------------------------

    def away(self, note: str = "") -> Reply:
        self.home.set_mode("away", note)
        return Reply("Away mode: only urgent news will interrupt you; the rest waits for `cos back`.")

    def quiet(self, on: bool = True) -> Reply:
        self.home.set_mode("quiet" if on else "normal")
        return Reply("Quiet mode: only decisions and urgent news." if on else "Quiet mode off.")

    def back(self) -> Reply:
        self.home.set_mode("normal")
        rows = self.decisions.drain_digest()
        if not rows:
            return Reply("Welcome back. Nothing happened that needed you.", [])
        return Reply("Welcome back. While you were away: " + "; ".join(r["message"] for r in rows[-8:]), rows)

    # -- running it --------------------------------------------------------------------

    def tick(self) -> Reply:
        with self.home.lock():
            summary = self.host.tick()
        return self._reply_for(summary)

    def learn(self) -> Reply:
        result = self.host.learning.run_pass()
        live = [a["name"] for a in result.get("adopted", []) if a["live"]]
        lines = [f"Looked at {len(result.get('patterns', []))} repeated kinds of work"]
        if live:
            lines.append(f"Switched on new playbooks: {', '.join(live)}")
        return Reply(self.host.reply(lines), result)

    # -- playbooks ------------------------------------------------------------------

    def playbooks(self, action: str = "list", name: str | None = None) -> Reply:
        books = self.playbook_registry
        if action == "list":
            rows = books.registry()
            if not rows:
                return Reply("No playbooks learned yet.", {})
            return Reply("\n".join(f"{n} v{r['version']} ({r['category'].replace('_', ' ')}): {'on' if r.get('active') else 'off'}" for n, r in rows.items()), rows)
        if name is None:
            raise ValueError("name the playbook")
        if action == "show":
            row = books.registry()[name]
            trail = "\n".join(f"# {e['at']} {e['event']}" for e in books.audit_trail(name)[-10:])
            return Reply(Path(row["path"]).read_text() + "\n" + trail, row)
        if action == "disable":
            books.set_active(name, False)
            return Reply(f"{name} is off; new requests will not use it.")
        if action == "enable":
            books.set_active(name, True)
            return Reply(f"{name} is on.")
        if action == "revert":
            row = books.revert(name)
            return Reply(f"{name} is back to version {row['version']}{'' if row.get('active') else ' and switched off'}.", row)
        raise ValueError(f"unknown playbook action `{action}`")


def _workers(home: Home) -> list[dict[str, Any]]:
    from .state.workers import Workers

    return Workers(home).all()
