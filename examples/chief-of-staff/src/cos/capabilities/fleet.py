"""The `fleet` capability: every effect the programs ask of the fleet.

A `tool` (spec section 9.4) whose verbs are declared with signatures in the
`.jev` modules and published here as a manifest, so a mismatch fails before
any model call. Reads (``skill_search``, ``landable``, ``landed``,
``pr_state``, ``authority``) always reach the world; every other verb is an effect and goes
through the wake's idempotency keys (``effects.EffectLog``), so a wake that is
run again after a crash never acts twice.
"""

from __future__ import annotations

import time
from pathlib import Path
from typing import TYPE_CHECKING, Any

from ..state.home import atomic_write
from ..state.inbox import Inbox, doorbell
from ..skills import SkillError

if TYPE_CHECKING:  # pragma: no cover
    from ..host import Host

VERBS: dict[str, tuple[list[str], str]] = {
    "worktree": (["id", "project"], "record"),
    "skill_search": (["id", "scout", "request", "k"], "record"),
    "ensure_skills": (["id", "worktree", "harness", "selected", "status", "uncertain"], "record"),
    "write_brief": (["id", "text"], "text"),
    "steer": (["id", "text"], "record"),
    "ring": (["id"], "bool"),
    "digest": (["text"], "none"),
    "landable": (["id"], "record"),
    "land": (["id"], "record"),
    "landed": (["id"], "record"),
    "pr_state": (["id"], "record"),
    "merge": (["id"], "record"),
    "authority": (["id"], "record"),
    "authorize": (["id", "answer"], "none"),
    "cleanup": (["id"], "record"),
    "relaunch": (["id"], "record"),
    "follow": (["id", "answer", "text"], "record"),
    "track_review": (["id", "handle"], "none"),
}
READS = {"skill_search", "landable", "landed", "pr_state", "authority"}


class Fleet:
    kind = "tool"
    manifest = {"verbs": {verb: {"params": params, "returns": ret} for verb, (params, ret) in VERBS.items()}}

    def __init__(self, host: "Host") -> None:
        self.host = host

    def begin(self, wake_id: str) -> None:
        self.host.effects.begin(wake_id)

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        method = getattr(self, f"v_{verb}", None)
        if method is None or verb not in VERBS:
            raise LookupError(f"`fleet` has no verb `{verb}`")
        positional = list(args.get("positional") or [])
        if verb == "ensure_skills":
            # The receipt and installed bytes must be reconciled even when a
            # previous wake recorded success before the worker started.
            return method(*positional)
        if verb in READS:
            return method(*positional)
        return self.host.effects.once(f"fleet.{verb}", positional, lambda: method(*positional))

    # -- helpers --------------------------------------------------------------

    def _worker(self, task_id: str) -> dict[str, Any]:
        worker = self.host.workers.get(task_id)
        if worker is None:
            raise LookupError(f"no worker `{task_id}`")
        return worker

    def _project(self, worker: dict[str, Any]) -> dict[str, Any]:
        return self.host.registry.project(worker["project"])

    # -- isolated copies and instructions ------------------------------------------

    def v_worktree(self, task_id: str, project_name: str) -> dict[str, Any]:
        project = self.host.registry.project(project_name)
        wt = self.host.delivery.create_worktree(project, task_id, self.host.home.worktrees(), self.host.worktree_backend())
        item = self.host.backlog.get(task_id)
        # The record exists from the moment the copy does, so a crash before
        # the spawn leaves something recovery can finish.
        self.host.workers.update(
            task_id,
            title=item["title"],
            project=project_name,
            kind=item.get("kind", "ship"),
            mode=project["mode"],
            yolo=bool(project.get("yolo")),
            worktree=wt["path"],
            branch=wt["branch"],
            base=wt["base"],
            phase="working",
            status_offset=0,
            nudges=0,
            relaunches=0,
        )
        return wt

    def v_write_brief(self, task_id: str, text: str) -> str:
        path = self.host.home.task_dir(task_id) / "brief.md"
        atomic_write(path, text)
        return str(path)

    def v_skill_search(self, task_id: str, scout: str, request: str, k: int) -> dict[str, Any]:
        """The recorded shortlist for one item; its playbook comes from the backlog, not the program."""
        try:
            return {**self.host.skills.search(self.host.backlog.get(task_id), scout, request, k, self.host.policy()), "blocked": None}
        except (SkillError, OSError) as error:
            return {"blocked": str(error)}

    def v_ensure_skills(self, task_id: str, worktree: str, harness: str, selected: list[str], status: str, uncertain: list[str]) -> dict[str, Any]:
        if self._worker(task_id).get("worktree") != worktree:
            raise ValueError(f"skill destination is not the worker worktree for {task_id}")
        try:
            return {**self.host.skills.ensure(task_id, worktree, harness, selected, status, uncertain), "blocked": None}
        except (SkillError, OSError) as error:
            return {"blocked": str(error)}

    # -- steering ------------------------------------------------------------

    def v_ring(self, task_id: str) -> bool:
        worker = self._worker(task_id)
        self.host.crew.adapter.call("send", {"positional": [worker["handle"], doorbell(self.host.home.inbox_dir(task_id))]}, "crew")
        self.host.workers.update(task_id, rung_at=time.time())
        return True

    def v_steer(self, task_id: str, text: str) -> dict[str, Any]:
        path = Inbox(self.host.home, task_id).post(text)
        worker = self.host.workers.get(task_id)
        rung = bool(worker and worker.get("handle")) and self.v_ring(task_id)
        return {"path": str(path), "rung": rung}

    def v_follow(self, task_id: str, answer: str, text: str) -> dict[str, Any]:
        """Carry out the owner's answer about a worker."""
        choice = str(answer or "").strip().lower()
        if choice.startswith("relaunch"):
            return self.host.relaunch(task_id, reset=True)
        if choice.startswith("stop"):
            self.host.stop_worker(task_id, "stopped by the owner")
            return {"stopped": True}
        return self.v_steer(task_id, f"Answer from the owner: {text or answer}")

    def v_track_review(self, task_id: str, handle: Any) -> None:
        self.host.workers.update(task_id, review_handle=handle, review_offset=0)

    # -- the person ------------------------------------------------------------

    def v_digest(self, text: str) -> None:
        self.host.decisions.digest(text)

    def v_authority(self, task_id: str) -> dict[str, Any]:
        worker = self._worker(task_id)
        return {"answer": worker.get("approval"), "granted": bool(worker.get("yolo")) or worker.get("approval") == "yes"}

    def v_authorize(self, task_id: str, answer: str) -> None:
        choice = str(answer or "").strip().lower()
        self.host.workers.update(task_id, approval="yes" if choice.startswith("y") else "no")

    # -- delivery ------------------------------------------------------------

    def v_landable(self, task_id: str) -> dict[str, Any]:
        worker = self._worker(task_id)
        if not Path(worker["worktree"]).is_dir():
            return {"ok": False, "reason": "the isolated copy is gone"}
        try:
            owned = self.host.skills.owned_paths(task_id, worker["worktree"])
        except SkillError as error:
            return {"ok": False, "reason": str(error)}
        return self.host.delivery.landable(self._project(worker), worker["worktree"], worker["branch"], owned)

    def v_land(self, task_id: str) -> dict[str, Any]:
        worker = self._worker(task_id)
        check = self.v_landable(task_id)
        if not check["ok"]:
            return {"landed": False, "reason": check["reason"]}
        owned = self.host.skills.owned_paths(task_id, worker["worktree"])
        result = self.host.delivery.land(self._project(worker), worker["worktree"], worker["branch"], owned)
        if result.get("landed"):
            self.host.workers.update(task_id, landed_head=result["head"])
        return result

    def v_landed(self, task_id: str) -> dict[str, Any]:
        """Proof that the work is where it belongs, checked in the world."""
        worker = self._worker(task_id)
        if worker.get("kind") == "scout":
            report = self.host.home.task_dir(task_id) / "report.md"
            return {"ok": report.exists(), "reason": "" if report.exists() else "no report yet"}
        if worker.get("pr"):
            merged = self.v_pr_state(task_id).get("state") == "MERGED"
            return {"ok": merged, "reason": "" if merged else "the pull request has not merged"}
        return self.host.delivery.landed(self._project(worker), worker["branch"])

    def v_pr_state(self, task_id: str) -> dict[str, Any]:
        worker = self._worker(task_id)
        url = worker.get("pr")
        if not url:
            return {"state": "NONE", "green": False, "skills_ok": True, "head_verified": True, "skill_paths": []}
        state = self.host.delivery.pr_state(url)
        skill_error = None
        head_verified = True
        head_paths: list[str] = []
        try:
            head_paths = self.host.skills.head_owned_paths(task_id, worker.get("worktree") or "", state.get("headRefOid"))
        except (SkillError, OSError) as error:
            head_verified = False
            skill_error = str(error)
        if head_paths:
            skill_error = f"pull request head contains host Skill files: {', '.join(head_paths)}"
        return {**state, "green": bool(state.get("green")) and skill_error is None,
                "skills_ok": skill_error is None, "head_verified": head_verified,
                "skill_paths": head_paths, "reason": skill_error or state.get("reason", "")}

    def v_merge(self, task_id: str) -> dict[str, Any]:
        worker = self._worker(task_id)
        gate = self.v_pr_state(task_id)
        if not gate["green"]:
            return {"merged": False, "reason": gate.get("reason") or "the pull request is not open and green"}
        head_oid = gate.get("headRefOid")
        return self.host.delivery.merge(worker["pr"], head_oid) if head_oid else self.host.delivery.merge(worker["pr"])

    def v_cleanup(self, task_id: str) -> dict[str, Any]:
        worker = self._worker(task_id)
        proof = self.v_landed(task_id)
        if not proof["ok"]:
            return {"removed": False, "reason": proof["reason"]}
        pr_merged = worker.get("kind") == "scout" or bool(worker.get("pr"))
        remote = self.v_pr_state(task_id) if worker.get("pr") else None
        if remote is not None and remote["state"] != "MERGED":
            return self._cleanup_failed(worker, "the pull request merge proof changed before cleanup")
        if remote is not None and not remote["head_verified"]:
            return self._cleanup_failed(worker, remote["reason"])
        for handle in (worker.get("handle"), worker.get("review_handle")):
            if handle:
                try:
                    self.host.crew.adapter.call("stop", {"positional": [handle]}, "crew")
                except Exception as error:  # noqa: BLE001 - adapter failure must keep files intact
                    return self._cleanup_failed(worker, f"could not stop agent before Skill cleanup: {error}")
        if Path(worker["worktree"]).exists():
            try:
                self.host.skills.remove_owned(task_id, worker["worktree"], keep_tracked=bool(worker.get("pr")))
            except SkillError as error:
                return self._cleanup_failed(worker, str(error))
            if remote is not None and remote["skill_paths"]:
                self.host.decisions.notify(f"Merged work for '{worker['title']}' includes host Skill files: {', '.join(remote['skill_paths'])}. Remove them in a follow-up change.")
        result = self.host.delivery.cleanup(self._project(worker), worker["worktree"], worker["branch"], self.host.worktree_backend(), pr_merged=pr_merged)
        self.host.workers.update(task_id, cleanup=result)
        if not result["removed"]:
            self.host.decisions.notify(f"Landed '{worker['title']}' but its isolated copy was kept: {result['reason']}")
        return result

    def _cleanup_failed(self, worker: dict[str, Any], reason: str) -> dict[str, Any]:
        result = {"removed": False, "reason": reason}
        self.host.workers.update(worker["id"], cleanup=result)
        self.host.decisions.notify(f"Landed '{worker['title']}' but its isolated copy was kept: {reason}")
        return result

    def v_relaunch(self, task_id: str) -> dict[str, Any]:
        return self.host.relaunch(task_id)


def report_path(host: "Host", task_id: str) -> Path:
    return host.home.task_dir(task_id) / "report.md"
