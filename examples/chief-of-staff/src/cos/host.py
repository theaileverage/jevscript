"""The host: wires the layers together and persists what episodes decide.

    watcher -> wake queue -> episode runner -> state store
                                   |
                         capability bindings (fleet, me, writer, crew)

The host never decides anything a judgment or a policy should decide. For
each wake it builds the snapshot the wake's task reads, runs one bounded
episode of ``cos.jev``, persists the episode's ``result`` into the state
store, and only then acknowledges the wake. A question to the owner parks the
episode; the answer resumes it (in memory, or from its recording after a
restart) and the wake is acknowledged when it finishes.
"""

from __future__ import annotations

import sys
import time
import secrets
from typing import Any

from . import backends
from .capabilities.agent import AdapterError, AgentBinding, from_config
from .capabilities.effects import EffectLog
from .capabilities.fleet import Fleet
from .capabilities.llm import TemplateWriter
from .capabilities.person import Person
from .delivery import Delivery
from .episode import Episode, Episodes, JevSession
from .jevbin import SetupError, jev_dir
from .learning import Learning
from .mates import Mates
from .state.backlog import Backlog
from .state.decisions import Decisions, desktop_notifier
from .state.home import Home, iso, now, read_json, write_json, request_id
from .state.ledger import Ledger, words_minutes
from .state.memory import Memory
from .state.threads import Threads
from .skills import SkillError, Skills
from .state.inbox import Inbox
from .state.registry import Registry
from .state.workers import Workers
from .wakes import WakeQueue
from .watcher import Watcher

ACTIVE_KINDS = ("", "working", "resolved")
#: Events that only wait; any other event moved the worker and earns a follow-up wake.
WAITING_EVENTS = {"carry_on", "awaiting_review", "checks_pending", "rering", "stay"}


class Host:
    def __init__(
        self,
        home: Home,
        *,
        session: JevSession | None = None,
        crew: Any = None,
        writer: Any = None,
        delivery: Delivery | None = None,
        terminal: backends.Backend | None = None,
        notify_desktop: bool | None = None,
        echo: bool = True,
    ) -> None:
        self.home = home.init()
        config = self.home.config
        self.registry = Registry(self.home)
        self.memory = Memory(self.home)
        self.backlog = Backlog(self.home)
        self.threads = Threads(self.home)
        self.ledger = Ledger(self.home)
        self.workers = Workers(self.home)
        notifier = desktop_notifier if (notify_desktop if notify_desktop is not None else config.get("desktop_notifications")) else None
        self.decisions = Decisions(self.home, notifier, echo=echo)
        self.delivery = delivery or Delivery(merge_flags=config["forge_merge_flags"])
        self.effects = EffectLog(self.home)
        self.skills = Skills(self.home)
        self.session = session or JevSession(self.home)
        self.crew = AgentBinding(crew if crew is not None else self._adapter("crew", "agent"), self.effects)
        self.writer = writer if writer is not None else (self._adapter("writer", "llm") if "writer" in config.get("adapters", {}) else TemplateWriter())
        self._terminal = terminal
        self.wakes = WakeQueue(self.home)
        self.watcher = Watcher(self)
        self.episodes = Episodes(self.home, self.session, self.ledger, self.decisions)
        self.fleet = Fleet(self)
        self.person = Person(self)
        self.learning = Learning(self)
        self.mates = Mates(self)
        self.program = self.session.load(jev_dir() / "cos.jev")

    # -- wiring -------------------------------------------------------------------

    def _adapter(self, name: str, kind: str) -> Any:
        entry = self.home.config.get("adapters", {}).get(name)
        if name == "crew" and not entry:
            entry = {"command": [sys.executable, "-m", "cos.terminal_agent"]}
        if not entry:
            raise SetupError(
                f"No `{name}` adapter is bound. Set adapters.{name}.command in {self.home.config_path} to a process "
                "that speaks the JSONL adapter protocol (for a dry run: `python -m cos.fake_agent`)."
            )
        log = self.home.state / "adapters" / f"{name}.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        return from_config(entry, kind, name, log=log)

    def terminal(self) -> backends.Backend:
        """The terminal backend the host uses for its own processes (second
        mates). Workers' terminals belong to the agent adapter."""
        if self._terminal is None:
            config = self.home.config
            self._terminal = backends.make(config["backend"], **config.get("backend_options", {}))
        return self._terminal

    def worktree_backend(self) -> Any:
        return backends.make("orca") if self.home.config.get("worktrees") == "orca" else None

    def bind(self) -> dict[str, Any]:
        return {"crew": self.crew, "me": self.person, "writer": self.writer, "fleet": self.fleet}

    def policy(self) -> dict[str, Any]:
        return {**self.home.config["policy"], "mode": self.home.mode}

    def log(self, message: str) -> None:
        print(f"[{iso()}] {message}", file=sys.stderr)

    def run_task(self, task: str, snapshot: dict[str, Any], subject: str, wake_id: str | None = None) -> Episode:
        episode = self.episodes.run(self.program, task, snapshot, self.bind(), subject=subject, wake_id=wake_id)
        if episode.problem:
            self.log(f"{task} {subject}: {episode.problem}")
        return episode

    def reply(self, lines: list[str]) -> str:
        """A short outcome summary for the person, written by the `llm`
        capability through ``escalation.summarize``."""
        if not lines:
            return "Nothing changed."
        episode = self.run_task("reply", {"facts": {"lines": lines}}, subject="reply")
        return str(episode.result or " ".join(lines))

    def close(self) -> None:
        for closer in (self.crew.close, getattr(self.writer, "close", None), self.program.close, self.session.close):
            if closer is not None:
                try:
                    closer()
                except Exception:  # noqa: BLE001 - shutting down
                    pass

    # -- requests ----------------------------------------------------------------

    def submit(self, text: str, *, after: list[str] | None = None, project: str | None = None, source: str = "cli", channel: str | None = None, message_id: str | None = None, reply_to: str | None = None, native_thread: str | None = None) -> dict[str, Any]:
        rid = request_id()
        request = {"id": rid, "message_id": f"m-{secrets.token_hex(10)}" if message_id is None else message_id, "reply_to": reply_to, "native_thread": native_thread, "channel": channel, "text": text, "after": after or [], "project": project, "source": source, "at": iso()}
        receipt = self.threads.enqueue(request)
        if receipt["request_id"] != rid:
            path = self.home.state / "requests" / f"{receipt['request_id']}.json"
            original = read_json(path, receipt["request"])
            if receipt["status"] == "pending" and not path.exists():
                write_json(path, original)
            return {**original, "duplicate": True, "receipt": receipt}
        write_json(self.home.state / "requests" / f"{rid}.json", request)
        return request

    # -- snapshots ------------------------------------------------------------------

    def fleet_lists(self) -> dict[str, Any]:
        return {
            "projects": [{"name": p["name"], "description": p["description"], "mode": p["mode"]} for p in self.registry.projects()],
            "homes": self.registry.homes(),
            "profiles": self.registry.profiles(),
            "playbooks": self.learning.playbooks.active(),
            "policy": self.policy(),
        }

    def snapshot_for(self, wake: dict[str, Any]) -> dict[str, Any] | None:
        kind = wake["kind"]
        if kind == "request":
            request = read_json(self.home.state / "requests" / f"{wake['subject']}.json", None)
            if request is None:
                return None
            request.setdefault("message_id", request["id"])
            receipt = self.threads.receipt_for(request)
            if receipt and receipt.get("status") == "done":
                self._reconcile_thread_task(receipt)
                (self.home.state / "requests" / f"{wake['subject']}.json").unlink(missing_ok=True)
                return None
            lists = self.fleet_lists()
            lists["profiles"] = [{"name": p["name"], "rule": p["rule"]} for p in lists["profiles"]]
            project_names = {p["name"] for p in lists["projects"]} | {"", request.get("project") or ""}
            contexts = []
            for project in sorted(project_names):
                scoped = {**request, "project": project or None, "effective_project": project or None}
                contexts.append({"project": project, "relation": self.threads.relation(scoped), "candidates": self.threads.candidates(scoped)})
            return {"wake": wake, "request": {"id": request["id"], "text": request["text"], "project": request.get("project"), "skip_playbooks": bool(request.get("skip_playbooks")), "thread_contexts": contexts}, **lists}
        if kind == "dispatch":
            items = self.backlog.snapshot()
            paths = {i["id"]: self.paths(i["id"]) for i in items}
            projects = {p["name"] for p in self.registry.projects()}
            return {
                "wake": wake,
                "backlog": [{**i, "deps": i.get("deps") or [], "notes": i.get("notes") or "", "thread_id": i.get("thread_id")} for i in items if i.get("project") in projects or i["status"] != "queued"],
                "now": now(),
                "prefs": self.memory.preferences(),
                "paths": paths,
                "skill_catalog": self.skills.snapshot(),
                **self.fleet_lists(),
            }
        if kind == "worker":
            worker = self.workers.get(wake["subject"])
            if worker is None or worker.get("retired"):
                return None
            return {"wake": wake, "worker": self.worker_snapshot(worker), "policy": self.policy()}
        return {"wake": wake, "policy": self.policy()}

    def paths(self, task_id: str) -> dict[str, str]:
        return {
            "status": str(self.home.status_path(task_id)),
            "inbox": str(self.home.inbox_dir(task_id)),
            "report": str(self.home.task_dir(task_id) / "report.md"),
            "review_status": str(self.home.status_path(f"{task_id}-review")),
        }

    def reviewer_for(self, worker: dict[str, Any]) -> dict[str, Any] | None:
        try:
            name = self.registry.project(worker["project"]).get("review") or self.home.config.get("review_profile")
        except KeyError:
            name = None
        if not name:
            return None
        found = [p for p in self.registry.profiles() if p["name"] == name]
        return found[0] if found else None

    def worker_snapshot(self, worker: dict[str, Any]) -> dict[str, Any]:
        report = self.home.task_dir(worker["id"]) / "report.md"
        kind = worker.get("status_kind", "")
        recent = [{"action": a["action"], "target": worker["id"], "args": a.get("args", "")} for a in worker.get("actions", [])[-6:]]
        return {
            "id": worker["id"],
            "worker_id": worker.get("worker_id"),
            "title": worker.get("title", worker["id"]),
            "project": worker.get("project"),
            "kind": worker.get("kind", "ship"),
            "mode": worker.get("mode", "local-only"),
            "yolo": bool(worker.get("yolo")),
            "handle": worker["handle"],
            "phase": worker.get("phase", "working"),
            "alive": bool(worker.get("alive", True)),
            "busy": bool(worker.get("busy")),
            "active": kind in ACTIVE_KINDS,
            "stale": bool(worker.get("stale")),
            "status_kind": kind,
            "status_new": bool(worker.get("status_new")),
            "last_status": worker.get("last_status", ""),
            "idle_minutes": round(worker.get("idle_minutes", 0), 1),
            "idle_words": f"the screen has not changed for {words_minutes(worker.get('idle_minutes', 0))}",
            "unacked": worker.get("unacked", 0),
            "unacked_minutes": round(worker.get("unacked_minutes", 0), 1),
            "nudges": worker.get("nudges", 0),
            "relaunches": worker.get("relaunches", 0),
            "pr": worker.get("pr"),
            "worktree": worker.get("worktree"),
            "branch": worker.get("branch"),
            "base": worker.get("base"),
            "reviewer": self.reviewer_for(worker),
            "review_text": worker.get("review_text") or "",
            "review_new": bool(worker.get("review_new")),
            "report_exists": report.exists(),
            "paths": self.paths(worker["id"]),
            "recent": recent,
            "screen": worker.get("screen", "")[-6000:],
        }

    # -- the episode loop -------------------------------------------------------------

    def handle(self, wake: dict[str, Any]) -> Episode | None:
        try:
            snapshot = self.snapshot_for(wake)
        except SkillError as error:
            if wake["kind"] != "dispatch":
                raise
            key = f"{wake['id']}:skill_catalog"
            self.decisions.record(key, f"Skill catalog blocks dispatch: {error}", ["retry", "dismiss"], {"wake": wake["id"]})
            self.wakes.pause(wake, key)
            return Episode("on_wake", wake["subject"], wake["id"], None, "", paused=key)
        if snapshot is None:
            self.wakes.ack(wake, "nothing to do")
            return None
        task = "on_wake" if wake["kind"] in ("request", "dispatch", "worker") else wake["kind"]
        episode = self.run_task(task, snapshot, subject=wake["subject"], wake_id=wake["id"])
        return self.settle(wake, episode)

    def settle(self, wake: dict[str, Any], episode: Episode) -> Episode:
        if episode.paused:
            self.wakes.pause(wake, episode.paused)
            return episode
        follow_up = False
        if episode.result is not None:
            follow_up = bool(self.persist(wake, episode))
        self.wakes.ack(wake, "persisted" if episode.result is not None else (episode.problem or "no result"))
        self.effects.forget(wake["id"])
        if follow_up:
            # The worker moved; its next step may be possible right away.
            self.wakes.push(wake["kind"], wake["subject"], ["continue after a transition"])
        if wake["kind"] == "request" and episode.result and (episode.result.get("route") in ("backlog", "playbook") or episode.result.get("item")):
            self.wakes.push("dispatch", "backlog", ["new work queued"])
        return episode

    def resume_answered(self) -> list[Episode]:
        """Resume every parked episode whose question has been answered."""
        done = []
        for decision in list(self.decisions.all().values()):
            key = decision["key"]
            if not key.endswith(":skills") or decision.get("answer") is None:
                continue
            task_id = key[:-len(":skills")]
            item = self.backlog.get(task_id)
            if item["status"] == "queued":
                if str(decision["answer"]).strip().lower().startswith("retry"):
                    self.backlog.release(task_id)
                    self.wakes.push("dispatch", "backlog", [f"Skill retry for {task_id}"])
                else:
                    worker = self.workers.get(task_id)
                    if worker and not worker.get("handle") and worker.get("worktree"):
                        try:
                            self.skills.remove_owned(task_id, worker["worktree"], allow_pending=True)
                            cleanup = self.delivery.cleanup(self.registry.project(worker["project"]), worker["worktree"], worker["branch"], self.worktree_backend())
                        except (SkillError, OSError) as error:
                            cleanup = {"removed": False, "reason": str(error)}
                        self.workers.update(task_id, cleanup=cleanup)
                        if cleanup["removed"]:
                            self.workers.retire(task_id, "cancelled")
                        else:
                            self.decisions.notify(f"Dismissed '{worker['title']}' but its isolated copy was kept: {cleanup['reason']}")
                    self.backlog.finish(task_id, "cancelled", "Skill installation dismissed")
            self.decisions.consume(key)
        for wake in self.wakes.pending():
            key = wake.get("paused")
            if not key:
                continue
            answer = (self.decisions.all().get(key) or {}).get("answer")
            if answer is None:
                continue
            if key.endswith(":skill_catalog"):
                self.decisions.consume(key)
                if str(answer).strip().lower().startswith("retry"):
                    self.wakes.resume(wake)
                else:
                    self.wakes.ack(wake, "Skill catalog dispatch dismissed")
                continue
            self.decisions.consume(key)
            parked = [r for r in self.episodes.parked_records() if r.get("wake_id") == wake["id"]]
            if not parked:
                self.wakes.ack(wake, "the parked run was lost")
                continue
            wake["paused"] = None
            episode = self.episodes.resume(wake["id"], str(answer), self.program, self.bind())
            done.append(self.settle(wake, episode))
        return done

    def tick(self) -> dict[str, Any]:
        summary: dict[str, Any] = {"at": iso(), "resumed": [], "handled": []}
        summary["resumed"] = [e.subject for e in self.resume_answered()]
        self.watcher.scan()
        for _ in range(25):  # a bounded number of wakes per tick
            ready = [w for w in self.wakes.pending() if not w.get("paused")]
            if not ready:
                break
            wake = ready[0]
            episode = self.handle(wake)
            summary["handled"].append({"kind": wake["kind"], "subject": wake["subject"], "result": episode.result if episode else None, "paused": episode.paused if episode else None})
        if self.learning.due():
            summary["learning"] = self.learning.run_pass()
        self.mates.tick()
        return summary

    # -- persisting what an episode decided --------------------------------------------

    def persist(self, wake: dict[str, Any], episode: Episode) -> bool:
        """Save what the episode decided; True when a follow-up wake is due."""
        kind = wake["kind"]
        if kind == "request":
            episode.result.update(self.persist_request(wake["subject"], episode.result, episode.recording))
        elif kind == "dispatch":
            self.persist_dispatch(episode.result, episode.recording)
            return bool(episode.result.get("more_ready"))
        elif kind == "worker":
            return self.persist_worker(wake, episode)
        return False

    def persist_request(self, request_id: str, d: dict[str, Any], recording: str) -> dict[str, Any]:
        path = self.home.state / "requests" / f"{request_id}.json"
        request = read_json(path, {"id": request_id, "text": d.get("text", "")})
        request.setdefault("message_id", request_id)
        route = d.get("route")
        outcome: dict[str, Any] = {"route": route}
        previous = self.threads.receipt_for(request)
        if previous and previous.get("status") == "done":
            self._reconcile_thread_task(previous)
            path.unlink(missing_ok=True)
            return {"thread_id": previous.get("thread_id"), "task_id": previous.get("task_id")}
        request["effective_project"] = d.get("project") or request.get("project")
        self.threads.receipt_for(request)
        if route == "continue":
            thread = self.threads.get(d["thread_id"])
            if thread is None or thread["scope"] != self.threads.scope(request):
                raise ValueError("invalid continuation thread")
            task_id = thread.get("task_id")
            if task_id:
                item = self.backlog.get(task_id)
                if item["status"] == "queued":
                    if request["message_id"] not in item.get("thread_messages", []):
                        self.backlog.update(task_id, notes=(item.get("notes") or "") + f"\nFollow-up in {thread['id']}: {request['text']}", thread_messages=item.get("thread_messages", []) + [request["message_id"]])
                elif item["status"] == "in_flight":
                    instruction = f"Continue conversation {thread['id']} on task {task_id}. Incoming message: {request['text']}"
                    Inbox(self.home, task_id).post_once(instruction, request["message_id"])
                    if self.workers.get(task_id) and self.workers.get(task_id).get("handle"):
                        self.fleet.v_ring(task_id)
                else:
                    existing = next((i for i in self.backlog.items() + self.backlog.done_history() if (i.get("request") or {}).get("id") == request_id), None)
                    followup = existing or self.backlog.add({"text": request["text"], "title": request["text"][:72], "project": item["project"], "kind": item["kind"], "effort": item["effort"], "profile": item["profile"], "notes": f"Conversation {thread['id']} continues after task {task_id}. Earlier context:\n{thread['summary']}", "request": request})
                    task_id = followup["id"]
                    outcome["item"] = task_id
            receipt = self.threads.apply(request, d, task_id if outcome.get("item") else None)
            outcome.update(thread_id=receipt["thread_id"], task_id=receipt["task_id"])
            if receipt["task_id"]:
                worker = self.workers.get(receipt["task_id"])
                outcome["worker_id"] = worker.get("worker_id") if worker else None
            if outcome.get("item") and self.backlog.get(outcome["item"])["status"] == "queued":
                self.backlog.update(outcome["item"], thread_id=receipt["thread_id"])
            path.unlink(missing_ok=True)
            self.ledger.record("route", request=request_id, **outcome)
            return outcome
        if route == "playbook":
            existing = next((i for i in self.backlog.items() + self.backlog.done_history() if (i.get("request") or {}).get("id") == request_id), None)
            plan = None if existing else self.learning.run_playbook(d["playbook"], request)
            if existing or (plan and plan.get("matches") and plan.get("project")):
                item = existing or self.backlog.add(
                    {
                        "text": request["text"],
                        "title": d.get("title"),
                        "project": plan["project"],
                        "kind": plan.get("kind", "ship"),
                        "effort": plan.get("effort", "low"),
                        "profile": plan.get("profile", "default"),
                        "notes": plan.get("notes", ""),
                        "playbook": d["playbook"],
                        "deps": request.get("after", []),
                        "request": {**request, "intake_recording": recording, "route": "playbook"},
                    }
                )
                outcome.update(item=item["id"], playbook=d["playbook"])
            else:
                write_json(path, {**request, "skip_playbooks": True})
                return {"route": "reassess"}  # the request file stays: a new wake assesses it fully
        elif route == "backlog":
            existing = next((i for i in self.backlog.items() + self.backlog.done_history() if (i.get("request") or {}).get("id") == request_id), None)
            item = existing or self.backlog.add(
                {
                    "text": request["text"],
                    "title": d.get("title"),
                    "project": d.get("project") or request.get("project"),
                    "kind": d.get("kind", "ship"),
                    "effort": d.get("effort", "medium"),
                    "profile": d.get("profile", "default"),
                    "deps": request.get("after", []),
                    "request": {**request, "intake_recording": recording, "decision": {"project": d.get("project"), "kind": d.get("kind")}},
                    "signals": d.get("signals", {}),
                }
            )
            outcome["item"] = item["id"]
        elif route == "memory":
            self.memory.remember_preference(request["text"])
            self.decisions.notify("Noted as a standing preference.")
        elif route == "status":
            from .bearings import headline

            self.decisions.notify(headline(self))
        elif route == "mate":
            self.mates.forward(d["mate"], request)
            outcome["mate"] = d["mate"]
        elif route == "declined":
            self.decisions.notify(f"Dropped '{d.get('title')}' as you asked.")
        else:
            self.decisions.notify(f"Could not place '{d.get('title')}': {d.get('reason') or 'it stayed unclear'}. Say it again with more detail.")
        if d.get("thread_route") in ("new", "continue") and route not in ("unclear", "declined"):
            if outcome.get("item"):
                request["effective_project"] = self.backlog.get(outcome["item"])["project"]
            thread_route = d["thread_route"]
            receipt = self.threads.apply(request, {"route": thread_route, "thread_id": d.get("thread_id"), "reason": d.get("reason") or thread_route}, outcome.get("item"))
            outcome.update(thread_route=thread_route, thread_id=receipt["thread_id"], task_id=receipt["task_id"])
            self._reconcile_thread_task(receipt)
        elif route == "ask_thread":
            self.threads.apply(request, {"route": "ask", "reason": d.get("reason")})
            outcome.update(thread_route="ask", thread_id=None, task_id=None)
        path.unlink(missing_ok=True)
        self.ledger.record("route", request=request_id, **outcome)
        return outcome

    def _reconcile_thread_task(self, receipt: dict[str, Any]) -> None:
        """Restore the task link if a crash followed the durable message receipt."""
        task_id, thread_id = receipt.get("task_id"), receipt.get("thread_id")
        if not task_id or not thread_id:
            return
        item = next((item for item in self.backlog.items() if item["id"] == task_id), None)
        if item is not None and item.get("thread_id") != thread_id:
            self.backlog.update(task_id, thread_id=thread_id)

    def persist_dispatch(self, result: dict[str, Any], recording: str) -> list[str]:
        started = []
        for s in result.get("started", []):
            if s.get("blocked"):
                key = f"{s['id']}:skills"
                self.decisions.record(key, f"Skill installation blocks '{s['id']}': {s['blocked']}", ["retry", "dismiss"], {"task": s["id"]})
                if self.backlog.get(s["id"])["status"] == "queued":
                    self.backlog.hold(s["id"], key)
                continue
            if self.backlog.get(s["id"])["status"] == "queued":
                self.backlog.start(s["id"])
            self.workers.update(
                s["id"],
                handle=s["handle"],
                brief=s["brief"],
                harness=s.get("harness"),
                model=s.get("model"),
                effort=s.get("effort"),
                profile=s.get("profile"),
                backend=s.get("backend"),
                phase="working",
                alive=True,
                started_at=now(),
                screen_changed_at=now(),
                dispatch_recording=recording,
                skills=s.get("skills", []),
                skill_selection=s.get("skill_selection"),
                skill_receipt=s.get("skill_receipt"),
            )
            started.append(s["id"])
        return started

    def persist_worker(self, wake: dict[str, Any], episode: Episode) -> bool:
        worker = self.workers.get(wake["subject"])
        if worker is None or worker.get("retired"):
            return False
        result = episode.result
        events = result.get("events", [])
        phase = result.get("phase") or worker.get("phase")
        actions = worker.get("actions", []) + [{"action": e, "args": ""} for e in events if not e.startswith("at_") and e not in WAITING_EVENTS]
        history = worker.get("history", []) + [{"at": iso(), "wake": wake["id"], "reasons": wake.get("reasons", []), "events": events, "phase": phase, "recording": episode.recording}]
        changes: dict[str, Any] = {"phase": phase, "status_new": False, "stale": False, "history": history[-40:], "actions": actions[-20:], "last_supervised": now()}
        if "nudge" in events:
            changes["nudges"] = worker.get("nudges", 0) + 1
        if phase != "review":
            changes["review_new"] = False
        if any(e in events for e in ("hand_over", "owner_decides", "ask_authority")):
            changes["escalations"] = worker.get("escalations", 0) + 1
        if worker.get("phase") == "review" and phase != "review" and worker.get("review_handle"):
            try:
                self.crew.adapter.call("stop", {"positional": [worker["review_handle"]]}, "crew")
            except AdapterError:
                pass
        worker = self.workers.update(worker["id"], **changes)
        if phase == "done":
            self.finish(worker["id"], "landed", verified=bool(result.get("verified")))
        elif phase == "shelved":
            self.finish(worker["id"], "shelved", verified=False)
        else:
            return any(not e.startswith("at_") and e not in WAITING_EVENTS for e in events)
        return False

    # -- lifecycle helpers ------------------------------------------------------------

    def relaunch(self, task_id: str, reset: bool = False) -> dict[str, Any]:
        worker = self.workers.get(task_id)
        if worker is None:
            raise LookupError(task_id)
        if worker.get("handle"):
            try:
                self.crew.adapter.call("stop", {"positional": [worker["handle"]]}, "crew")
            except AdapterError:
                pass
        named = {
            "prompt": f"You were relaunched. Read {worker['brief']} again, check `git status` and `git log` in {worker['worktree']}, and continue where the work stopped.",
            "in": {"path": worker["worktree"]},
            "name": task_id,
            "harness": worker.get("harness"),
            "model": worker.get("model"),
            "effort": worker.get("effort"),
            "backend": worker.get("backend"),
        }
        handle = self.crew.adapter.call("spawn", {"positional": [], "named": {k: v for k, v in named.items() if v is not None}}, "crew")
        if isinstance(handle, dict) and "$jev" not in handle:
            handle = {**handle, "$jev": "handle", "capability": "crew"}
        relaunches = 0 if reset else worker.get("relaunches", 0) + 1
        self.workers.update(task_id, handle=handle, relaunches=relaunches, alive=True, screen_changed_at=now(), nudges=0 if reset else worker.get("nudges", 0))
        return {"relaunched": True, "relaunches": relaunches}

    def stop_worker(self, task_id: str, why: str) -> None:
        worker = self.workers.get(task_id)
        if worker and worker.get("handle"):
            try:
                self.crew.adapter.call("stop", {"positional": [worker["handle"]]}, "crew")
            except AdapterError:
                pass
        self.finish(task_id, "stopped", verified=False)
        self.decisions.notify(f"Stopped '{(worker or {}).get('title', task_id)}': {why}. Its isolated copy is kept.")

    def finish(self, task_id: str, outcome: str, verified: bool) -> None:
        worker = self.workers.get(task_id) or {"id": task_id}
        item = self.backlog.get(task_id)
        status = {"landed": "done", "shelved": "cancelled"}.get(outcome, "failed")
        self.backlog.finish(task_id, status, outcome)
        self.workers.retire(task_id, outcome)
        request = item.get("request") or {}
        decision = request.get("decision") or {}
        self.ledger.task(
            id=task_id,
            request_id=request.get("id"),
            request=item.get("text", ""),
            trigger=item.get("title"),
            project=item.get("project"),
            kind=item.get("kind"),
            effort=item.get("effort"),
            profile=item.get("profile"),
            playbook=item.get("playbook"),
            route=request.get("route", "assess"),
            intake_project=decision.get("project"),
            intake_kind=decision.get("kind"),
            outcome=outcome,
            verified=verified,
            minutes=round((now() - worker.get("started_at", now())) / 60, 2),
            nudges=sum(1 for a in worker.get("actions", []) if a["action"] == "nudge"),
            relaunches=worker.get("relaunches", 0),
            escalations=worker.get("escalations", 0),
            obstacle=worker.get("obstacle", ""),
            intake_recording=request.get("intake_recording"),
            recordings=[h["recording"] for h in worker.get("history", [])] + [r for r in [worker.get("dispatch_recording")] if r],
        )
        self.mates.report_up(task_id, outcome, item.get("title", task_id))

    # -- session start ------------------------------------------------------------

    def recover(self) -> list[str]:
        """Session start: reconcile durable records with live reality, once.
        Pending wakes stay queued and run again; parked questions wait for
        their answers; a copy whose spawn never finished is dispatched again."""
        notes = []
        for worker in self.workers.all():
            if not worker.get("handle"):
                item = self.backlog.get(worker["id"])
                if item["status"] not in ("queued", "in_flight"):
                    continue
                if item["status"] == "in_flight":
                    self.backlog.update(worker["id"], status="queued")
                notes.append(f"{worker['id']}: dispatching again (the previous start did not finish)")
        for item in self.backlog.items():
            if item["status"] == "in_flight" and self.workers.get(item["id"]) is None:
                self.backlog.update(item["id"], status="queued")
                notes.append(f"{item['id']}: in flight with no worker record; queued again")
        pending = self.wakes.pending()
        if pending:
            notes.append(f"{len(pending)} wakes carried over from the last session")
        return notes

    def watch(self, interval: float | None = None, max_ticks: int | None = None) -> None:
        interval = interval if interval is not None else float(self.home.config["poll_seconds"])
        ticks = 0
        while max_ticks is None or ticks < max_ticks:
            try:
                summary = self.tick()
                if summary.get("handled") or summary.get("resumed"):
                    self.log(f"handled {len(summary['handled'])} wakes, resumed {len(summary['resumed'])}")
            except SetupError:
                raise
            except Exception as error:  # noqa: BLE001 - one bad wake must not stop the watch
                self.log(f"tick failed: {error!r}")
            ticks += 1
            if max_ticks is None or ticks < max_ticks:
                time.sleep(interval)
