"""The watcher: zero-token polling and code-only triage.

The watcher checks durable state cheaply and asks no
model. New request files, a backlog with work that could start, and each
worker's observation (through the agent adapter), status log, inbox and
decisions become wakes in the durable queue; benign changes (a busy worker's
screen moving, a heartbeat with nothing new) are absorbed here and never cost
a Jev call.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from . import backends
from .capabilities.agent import AdapterError
from .state.home import now
from .state.inbox import Inbox, StatusLog

if TYPE_CHECKING:  # pragma: no cover
    from .host import Host


class Watcher:
    def __init__(self, host: "Host") -> None:
        self.host = host

    def scan(self) -> list[dict[str, Any]]:
        host = self.host
        pushed = []
        for path in sorted((host.home.state / "requests").glob("*.json")):
            pushed.append(host.wakes.push("request", path.stem, ["new request"]))
        if self._dispatch_due():
            pushed.append(host.wakes.push("dispatch", "backlog", ["work may start"]))
        for worker in host.workers.all():
            if not worker.get("handle") or host.wakes.find("worker", worker["id"]):
                continue
            reasons = self.observe(worker)
            if reasons:
                pushed.append(host.wakes.push("worker", worker["id"], reasons))
        return pushed

    def _dispatch_due(self) -> bool:
        items = self.host.backlog.items()
        stamp = [(i["id"], i["status"], bool(i.get("hold"))) for i in items]
        queued = [i for i in items if i["status"] == "queued"]
        if not queued:
            return False
        gate_passed = any(i.get("hold") and i["hold"].get("until") and i["hold"]["until"] <= now() for i in queued)
        changed = stamp != getattr(self, "_last_backlog", None)
        self._last_backlog = stamp
        return changed or gate_passed

    def observe(self, worker: dict[str, Any]) -> list[str]:
        """Read one worker without any model and record what changed.
        Returns the reasons it needs an episode, or ``[]`` when it does not."""
        host = self.host
        policy = host.policy()
        try:
            obs = host.crew.observe(worker["handle"])
        except AdapterError as error:
            host.log(f"observe {worker['id']}: {error}")
            return []
        changes: dict[str, Any] = {"screen": str(obs.get("tail") or "")[-6000:]}
        reasons: list[str] = []
        lines, offset = StatusLog(host.home, worker["id"]).read(worker.get("status_offset", 0))
        if lines:
            last = lines[-1]
            changes.update(status_offset=offset, status_kind=last.kind, last_status=last.text, status_new=True)
            if last.kind in ("working", "resolved"):
                changes["nudges"] = 0
            if last.url:
                changes["pr"] = last.url
            if last.kind in ("needs-decision", "blocked", "failed"):
                changes["obstacle"] = last.text
            reasons.append(f"status:{last.kind}")
        digest = backends.screen_hash(changes["screen"])
        if digest != worker.get("screen_hash"):
            changes.update(screen_hash=digest, screen_changed_at=now())
        idle = (now() - changes.get("screen_changed_at", worker.get("screen_changed_at", now()))) / 60
        alive = obs.get("status") != "exited"
        busy = obs.get("busy")
        changes.update(alive=alive, busy=bool(busy) if busy is not None else obs.get("status") == "running", idle_minutes=idle)
        if not alive and worker.get("alive", True):
            reasons.append("session ended")
        kind = changes.get("status_kind", worker.get("status_kind", ""))
        if alive and kind not in ("done", "failed") and idle >= policy["stall_minutes"] and now() - worker.get("stale_woken_at", 0) >= policy["stall_minutes"] * 60:
            reasons.append("quiet too long")
            changes["stale_woken_at"] = now()
            changes["stale"] = True
        unacked = Inbox(host.home, worker["id"]).unacked()
        oldest = max((u["age_minutes"] for u in unacked), default=0)
        changes.update(unacked=len(unacked), unacked_minutes=oldest)
        if unacked and oldest >= policy["rering_minutes"] and now() - worker.get("rung_at", 0) >= policy["rering_minutes"] * 60:
            reasons.append("instruction not acknowledged")
        if worker.get("phase") == "review":
            if worker.get("review_handle"):
                try:  # the lead reviewer is watched like any worker
                    if host.crew.observe(worker["review_handle"]).get("status") == "exited" and not worker.get("review_text"):
                        reasons.append("reviewer session ended")
                except AdapterError as error:
                    host.log(f"observe reviewer of {worker['id']}: {error}")
            review_lines, review_offset = StatusLog(host.home, f"{worker['id']}-review").read(worker.get("review_offset", 0))
            verdicts = [l for l in review_lines if l.kind in ("done", "blocked", "failed")]
            if verdicts:
                changes.update(review_offset=review_offset, review_text=verdicts[-1].text, review_new=True)
                reasons.append("review verdict")
        if worker.get("phase") == "approved" and (worker.get("pr") or changes.get("pr")) and now() - worker.get("pr_polled_at", 0) >= 300:
            reasons.append("pull request poll")
            changes["pr_polled_at"] = now()
        host.workers.update(worker["id"], **changes)
        return reasons
