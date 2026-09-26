"""Delivery: isolated copies, landing, pull requests and safe cleanup.

- ``create_worktree`` gives each task its own ``git worktree`` on a fresh
  branch from the project's default branch (or asks Orca for one).
- ``landable``/``land`` provide guarded local landing: the branch
  must be committed, clean and a strict fast-forward of the default branch;
  landing never merges, rebases or forces.
- ``pr_state``/``merge`` watch and merge a pull request through ``gh``; the
  program decides when merging is authorized (yolo or the owner's yes).
- ``cleanup`` preserves unlanded work: it refuses a
  dirty copy, a branch not contained in the default branch, and never passes
  ``--force``. A refusal is reported, never bypassed.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any, Callable

Runner = Callable[..., subprocess.CompletedProcess]


def _run(argv: list[str], timeout: float = 60.0) -> subprocess.CompletedProcess:
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout, check=False)


class Delivery:
    def __init__(self, run: Runner = _run, merge_flags: list[str] | None = None) -> None:
        self._run = run
        self.merge_flags = merge_flags or ["--squash"]

    def git(self, repo: str, *args: str) -> subprocess.CompletedProcess:
        return self._run(["git", "-C", repo, *args])

    def _ok(self, repo: str, *args: str) -> str:
        result = self.git(repo, *args)
        if result.returncode != 0:
            raise RuntimeError(f"git {' '.join(args[:2])} failed: {(result.stderr or result.stdout).strip()}")
        return result.stdout.strip()

    # -- isolated copies ------------------------------------------------------------

    def create_worktree(self, project: dict[str, Any], task_id: str, root: Path, backend: Any = None) -> dict[str, Any]:
        branch = f"{project.get('branch_prefix', 'cos/')}{task_id}"
        base = project.get("default_branch", "main")
        if backend is not None and getattr(backend, "provides_worktrees", False):
            return backend.create_worktree(project["path"], branch, base)
        path = root / project["name"] / task_id
        if path.exists():  # a crashed dispatch left it: reuse, never recreate
            return {"path": str(path), "branch": branch, "base": base}
        path.parent.mkdir(parents=True, exist_ok=True)
        exists = self.git(project["path"], "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}").returncode == 0
        if exists:
            self._ok(project["path"], "worktree", "add", str(path), branch)
        else:
            self._ok(project["path"], "worktree", "add", "-b", branch, str(path), base)
        return {"path": str(path), "branch": branch, "base": base}

    # -- local landing ------------------------------------------------------------

    def landable(self, project: dict[str, Any], worktree: str, branch: str, ignored_paths: list[str] | None = None) -> dict[str, Any]:
        repo, base = project["path"], project.get("default_branch", "main")
        if not Path(worktree).exists():
            return {"ok": False, "reason": "the isolated copy is gone"}
        # The caller has already verified receipt-owned Skill bytes. Ignore
        # only those exact untracked paths; staged or edited paths stay dirty.
        owned = set(ignored_paths or [])
        entries = self.git(worktree, "status", "--porcelain=v1", "-z", "--untracked-files=all").stdout.split("\0")
        dirty = [entry for entry in entries if entry and not (entry.startswith("?? ") and entry[3:] in owned)]
        if dirty:
            return {"ok": False, "reason": "there are uncommitted changes"}
        ahead = self.git(repo, "rev-list", "--count", f"{base}..{branch}").stdout.strip()
        if ahead in ("", "0"):
            return {"ok": False, "reason": "the branch has no commits beyond the main branch"}
        if self.git(repo, "merge-base", "--is-ancestor", base, branch).returncode != 0:
            return {"ok": False, "reason": f"the branch does not contain the latest {base}; rebase it"}
        return {"ok": True, "reason": "", "ahead": int(ahead)}

    def land(self, project: dict[str, Any], worktree: str, branch: str, ignored_paths: list[str] | None = None) -> dict[str, Any]:
        check = self.landable(project, worktree, branch, ignored_paths)
        if not check["ok"]:
            return {"landed": False, "reason": check["reason"]}
        repo, base = project["path"], project.get("default_branch", "main")
        current = self.git(repo, "symbolic-ref", "--quiet", "--short", "HEAD").stdout.strip()
        if current == base:
            if self.git(repo, "status", "--porcelain", "--untracked-files=no").stdout.strip():
                return {"landed": False, "reason": f"the project's own copy of {base} has uncommitted changes"}
            result = self.git(repo, "merge", "--ff-only", branch)
        else:
            # `fetch . a:b` refuses anything but a fast-forward.
            result = self.git(repo, "fetch", ".", f"{branch}:{base}")
        if result.returncode != 0:
            return {"landed": False, "reason": (result.stderr or result.stdout).strip()}
        head = self._ok(repo, "rev-parse", base)
        return {"landed": True, "head": head, "reason": ""}

    def landed(self, project: dict[str, Any], branch: str) -> dict[str, Any]:
        """Proof of landing: the branch's commits are all in the main branch."""
        repo, base = project["path"], project.get("default_branch", "main")
        if self.git(repo, "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}").returncode != 0:
            return {"ok": False, "reason": f"the branch {branch} is gone"}
        if self.git(repo, "merge-base", "--is-ancestor", branch, base).returncode != 0:
            return {"ok": False, "reason": f"the branch is not in {base}"}
        return {"ok": True, "reason": "", "head": self._ok(repo, "rev-parse", base)}

    # -- pull requests --------------------------------------------------------------

    def pr_state(self, url: str) -> dict[str, Any]:
        result = self._run(["gh", "pr", "view", url, "--json", "state,isDraft,statusCheckRollup,mergeable,headRefOid"])
        if result.returncode != 0:
            return {"state": "UNKNOWN", "green": False, "reason": result.stderr.strip()}
        data = json.loads(result.stdout)
        checks = data.get("statusCheckRollup") or []
        conclusions = [(c.get("conclusion") or c.get("state") or "").upper() for c in checks]
        green = (
            not data.get("isDraft")
            and all(c in ("SUCCESS", "NEUTRAL", "SKIPPED") for c in conclusions)
            and data.get("mergeable") != "CONFLICTING"
        )
        return {"state": data.get("state", "UNKNOWN"), "green": green, "checks": len(conclusions), "headRefOid": data.get("headRefOid")}

    def merge(self, url: str, expected_head_oid: str | None = None) -> dict[str, Any]:
        state = self.pr_state(url)
        if state["state"] != "OPEN" or not state["green"]:
            return {"merged": False, "reason": "the pull request is not open and green"}
        if expected_head_oid and state.get("headRefOid") != expected_head_oid:
            return {"merged": False, "reason": "the pull request head changed after Skill verification"}
        matched_head = ["--match-head-commit", expected_head_oid] if expected_head_oid else []
        result = self._run(["gh", "pr", "merge", url, *self.merge_flags, *matched_head])
        return {"merged": result.returncode == 0, "reason": result.stderr.strip()}

    # -- cleanup that never destroys unlanded work ------------------------------------

    def cleanup(self, project: dict[str, Any], worktree: str, branch: str, backend: Any = None, pr_merged: bool = False) -> dict[str, Any]:
        repo, base = project["path"], project.get("default_branch", "main")
        path = Path(worktree)
        if path.exists():
            if self.git(worktree, "status", "--porcelain").stdout.strip():
                return {"removed": False, "reason": "the isolated copy has uncommitted changes"}
            landed = self.git(repo, "merge-base", "--is-ancestor", branch, base).returncode == 0
            if not landed and not pr_merged:
                return {"removed": False, "reason": f"the branch is not in {base} yet"}
            if backend is not None and getattr(backend, "provides_worktrees", False):
                backend.remove_worktree(worktree)
            else:
                removed = self.git(repo, "worktree", "remove", worktree)  # no --force, ever
                if removed.returncode != 0:
                    return {"removed": False, "reason": removed.stderr.strip()}
        # -d (not -D) refuses a branch whose commits are not merged.
        self.git(repo, "branch", "-d", branch)
        return {"removed": True, "reason": ""}
